use serde_json::Value;

use super::{ParsedLine, ProgressEvent, WireEvent, has_status_prefix, is_canned_progress};

pub(super) fn parse_event(event: WireEvent) -> ParsedLine {
    let description = tool_description(&event);
    let text = event_text(&event);
    let kind = event.kind.unwrap_or_default();
    let id = event
        .tool_call_id
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| kind.clone());
    let name = event
        .tool_name
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| kind.clone());
    match kind.as_str() {
        "tool_running" | "tool_start" | "tool_started" => {
            ParsedLine::Progress(ProgressEvent::ToolStarted {
                id,
                name,
                description,
            })
        }
        "tool_completed" | "tool_complete" | "tool_done" => {
            ParsedLine::Progress(ProgressEvent::ToolCompleted { id, name })
        }
        "tool_failed" | "tool_error" => ParsedLine::Progress(ProgressEvent::ToolFailed {
            id,
            name,
            error: nonempty(event.error).or_else(|| nonempty(event.message)),
        }),
        "thinking_delta" => match text {
            Some(text) if has_status_prefix(text.trim()) => {
                ParsedLine::Progress(ProgressEvent::Status(text))
            }
            Some(text) if is_canned_progress(&text) => ParsedLine::Ignored,
            Some(text) => ParsedLine::Progress(ProgressEvent::Thought(text)),
            None => ParsedLine::Ignored,
        },
        // Muse Spark `thinking_end` is a full snapshot, not a delta. Replaying it
        // after streamed `thinking_delta` caused CC Thought-for flicker
        // (9b9cffc). Still accept it when Muse skipped deltas for the unit.
        "thinking_end" => match text {
            Some(text) if is_canned_progress(&text) => ParsedLine::Ignored,
            Some(text) if text.trim().is_empty() => ParsedLine::Ignored,
            Some(text) => ParsedLine::Progress(ProgressEvent::ThoughtEnd(text)),
            None => ParsedLine::Ignored,
        },
        "text_delta" | "message_update" => match text {
            Some(text) => ParsedLine::Progress(ProgressEvent::Message(text)),
            None => ParsedLine::Ignored,
        },
        "turn_start"
        | "model_request_start"
        | "run_start"
        | "message_start"
        | "model_trace"
        | "thinking_start"
        | "model_request_end"
        | "message_end"
        | "turn_end"
        | "run_end"
        | "api_retry"
        | "tool_queued"
        | "tool_queue"
        | "tool_waiting"
        | "" => ParsedLine::Ignored,
        other => ParsedLine::Progress(ProgressEvent::Note(match nonempty(event.message) {
            Some(message) => format!("{other}: {message}"),
            None => other.to_owned(),
        })),
    }
}

pub(super) fn event_text(event: &WireEvent) -> Option<String> {
    nonempty(event.text.clone())
        .or_else(|| nonempty(event.delta.clone()))
        .or_else(|| nonempty(event.message.clone()))
}

pub(super) fn tool_description(event: &WireEvent) -> Option<String> {
    nonempty(event.description.clone())
        .or_else(|| nonempty(event.query.clone()))
        .or_else(|| preview_arg(event.arguments.as_ref()))
}

#[rustfmt::skip]
pub(super) fn preview_arg(value: Option<&Value>) -> Option<String> {
    let Value::Object(map) = value? else { return None };
    ["query", "q", "url", "uri", "path", "file_path", "pattern", "command"]
        .into_iter()
        .find_map(|key| map.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| {
            let mut chars = text.chars();
            let head: String = chars.by_ref().take(80).collect();
            if chars.next().is_some() { format!("{head}…") } else { head }
        })
}

pub(super) fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

pub(super) fn error_text(error: Option<Value>, message: Option<String>) -> Option<String> {
    if let Some(message) = nonempty(message) {
        return Some(message);
    }
    match error? {
        Value::String(text) => nonempty(Some(text)),
        Value::Object(object) => object
            .get("message")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| serde_json::to_string(&Value::Object(object)).ok()),
        other => Some(other.to_string()),
    }
}
