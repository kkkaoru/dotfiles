//! Strip adapter-only WIP noise from committed assistant segments.

use serde_json::{Value, json};

use crate::anthropic::Segment;

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
        .filter_map(|block| {
            (block.get("type").and_then(Value::as_str) == Some("text"))
                .then(|| block.get("text").and_then(Value::as_str).unwrap_or(""))
        })
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

pub(super) const LIVE_PROSE_CHAR_LIMIT: usize = 280;

pub(super) fn compact_live_prose(text: &str) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(LIVE_PROSE_CHAR_LIMIT).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Cursor/Claude worker filler that collapses Claude Code 2.1 to repeating
/// `Thought for Xs` plus mechanical "Working on your request" lines.
pub(super) fn is_canned_worker_filler(text: &str) -> bool {
    let lower = normalize_filler_case(text);
    if lower.is_empty() {
        return false;
    }
    lower.starts_with("thought for ")
        || lower.starts_with("nucleating")
        || CANNED_FILLER_NEEDLES
            .iter()
            .any(|needle| lower.contains(needle))
}

fn normalize_filler_case(text: &str) -> String {
    text.trim()
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{2032}' => '\'',
            other => other,
        })
        .collect::<String>()
        .to_ascii_lowercase()
}

const CANNED_FILLER_NEEDLES: &[&str] = &[
    "working on your request",
    "i'll gather what i need",
    "put together the result",
    "continuing with the next step",
    "audit the local ctx",
    "local history is indexed",
    "scanning local history",
    "gathering the records for your report",
    "locating local session evidence",
    "pull the evidence needed",
    "pulling event evidence",
    "pulling the provenance",
    "checking for provider history",
];

pub(super) fn is_provider_status_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    trimmed.starts_with('▶')
        || trimmed.starts_with('✓')
        || trimmed.starts_with('✗')
        || trimmed.starts_with("… still working")
        || trimmed.starts_with("Claudex is still working")
        || trimmed.starts_with("Plan ")
        || trimmed.starts_with("Plan:")
        || trimmed.starts_with('●')
        || trimmed.starts_with('◎')
        || trimmed.starts_with('○')
        || lower.starts_with("status:")
        || trimmed.starts_with("SubAgent starting:")
        || trimmed.starts_with("SubAgent started:")
        || trimmed.starts_with("SubAgent finished")
        || trimmed.starts_with("SubAgent completed")
        || trimmed.starts_with("Retrying provider request")
        || trimmed.starts_with("Session mode:")
        || trimmed.starts_with("Session:")
        || trimmed.starts_with("🔎 WebSearch:")
}

/// Muse Spark often emits `Status: …Status: …` without newlines. Keep only the last.
pub(super) fn latest_worker_status(delta: &str) -> Option<String> {
    let trimmed = delta.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !lower.starts_with("status:") {
        return None;
    }
    let last = lower.rmatch_indices("status:").next()?.0;
    let status = compact_live_prose(trimmed[last..].trim());
    Some(format!("{}\n", status.trim_end()))
}

/// Drop prior `Status:` chrome so mid-turn updates replace instead of stacking.
pub(super) fn strip_worker_status_lines(text: &str) -> String {
    let kept = text
        .lines()
        .filter(|line| !line.trim_start().to_ascii_lowercase().starts_with("status:"))
        .collect::<Vec<_>>();
    if kept.iter().all(|line| line.trim().is_empty()) {
        return String::new();
    }
    let joined = kept.join("\n");
    if text.ends_with('\n') && !joined.ends_with('\n') {
        format!("{joined}\n")
    } else {
        joined
    }
}

#[cfg(test)]
#[path = "sanitize_tests.rs"]
mod tests;
