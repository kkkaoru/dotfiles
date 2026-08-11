use serde_json::Value;

use super::super::MessagesRequest;
use super::DEFAULT_NOTIFICATION_TEXT;

pub(super) fn notification_ack_text(request: &MessagesRequest) -> String {
    request
        .messages
        .iter()
        .rev()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content"))
        .filter_map(notification_text_from_content)
        .next()
        .unwrap_or_else(|| DEFAULT_NOTIFICATION_TEXT.to_owned())
}

pub(super) fn notification_text_from_content(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .filter(|text| !super::super::is_monitor_hint_text(text))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    if let Some(body) = lifecycle_body(&text, "agent-message") {
        return Some(sanitize_visible_text(body));
    }
    let body = lifecycle_body(&text, "task-notification")?;
    let mut parts = Vec::new();
    // Claude Code renders status in its task panel. Only the user-facing
    // summary and result belong in the assistant turn; exposing `completed`
    // or `stopped` as prose leaks transport metadata into the main session.
    for field in ["summary", "result"] {
        if let Some(value) = xml_field(body, field)
            && !value.trim().is_empty()
            && !parts.iter().any(|part: &String| part == value.trim())
        {
            parts.push(sanitize_visible_text(value));
        }
    }
    let summary = if parts.is_empty() {
        sanitize_visible_text(body)
    } else {
        parts.join("\n\n")
    };
    Some(enrich_task_notification_ack(&summary))
}

pub(super) fn enrich_task_notification_ack(summary: &str) -> String {
    let lower = summary.to_ascii_lowercase();
    let mut text = summary.to_owned();
    if lower.contains("previous session") || lower.contains("no completion record") {
        text.push_str(
            "\n\nClaudex: these are historical previous-session task IDs. Do not TaskStop them. \
Inspect saved transcripts or output files if needed; leave live workers alone and continue \
orchestration.",
        );
    }
    if lower.contains("no assistant messages found")
        || (lower.contains("agent \"") && lower.contains(" failed:"))
    {
        text.push_str(
            "\n\nClaudex: do not cascade TaskStop across unrelated in-flight scopes. Replace only \
this failed lane if the work is still unresolved, using a different available worker; never \
duplicate an in-flight scope key.",
        );
    }
    if lower.contains("was stopped by claude") {
        text.push_str(
            "\n\nClaudex: acknowledge the stop; relaunch the same scope key only when that work is \
still unresolved and no peer already covers it.",
        );
    }
    text
}

pub(super) fn lifecycle_body<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let opening = format!("<{tag}");
    let start = text.find(&opening)?;
    let opening_end = text[start..].find('>')? + start + 1;
    let closing = format!("</{tag}>");
    let end = text[opening_end..].find(&closing)? + opening_end;
    Some(&text[opening_end..end])
}

pub(super) fn xml_field<'a>(text: &'a str, field: &str) -> Option<&'a str> {
    lifecycle_body(text, field)
}

pub(super) fn sanitize_visible_text(text: &str) -> String {
    text.replace("<agent-message", "agent message")
        .replace("</agent-message>", "")
        .replace("<task-notification", "task notification")
        .replace("</task-notification>", "")
        .trim()
        .to_owned()
}

