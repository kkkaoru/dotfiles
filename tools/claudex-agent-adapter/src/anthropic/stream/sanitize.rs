//! Strip adapter-only WIP noise from committed assistant segments.

use serde_json::{Value, json};

/// Events that count as decoded activity for Claude Code's idle watchdog.
pub(super) fn is_visible_activity_event(event: &Value) -> bool {
    matches!(
        event.get("method").and_then(Value::as_str),
        Some(
            "item/agentMessage/delta"
                | "item/reasoning/summaryTextDelta"
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
                    .filter(|line| !is_provider_status_line(line.trim()))
                    .collect::<Vec<_>>()
                    .join("\n");
                block["text"] = json!(cleaned);
            }
            true
        }
        Some("thinking") => {
            let Some(thinking) = block.get("thinking").and_then(Value::as_str) else {
                return true;
            };
            let cleaned = thinking.replace(ZWSP, "").replace(STATUS, "");
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
        .all(|line| line.trim().is_empty() || is_provider_status_line(line.trim()))
}

fn is_provider_status_line(line: &str) -> bool {
    line.starts_with('▶')
        || line.starts_with('✓')
        || line.starts_with('✗')
        || line.starts_with("Plan ")
        || line.starts_with("Plan:")
        || line.starts_with('●')
        || line.starts_with('◎')
        || line.starts_with('○')
        || line.starts_with("SubAgent starting:")
        || line.starts_with("SubAgent started:")
        || line.starts_with("SubAgent finished")
        || line.starts_with("SubAgent completed")
        || line.starts_with("Retrying provider request")
        || line.starts_with("Session mode:")
        || line.starts_with("Session:")
        || line.starts_with("🔎 WebSearch:")
}
