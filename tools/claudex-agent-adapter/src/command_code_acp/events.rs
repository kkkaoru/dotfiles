use serde::Deserialize;
use serde_json::Value;

mod chrome;
mod progress;
pub use progress::{
    progress_to_updates, result_is_error, result_message, turn_cancelled_updates,
};
use chrome::{has_status_prefix, is_canned_progress};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProgressEvent {
    Started {
        model: String,
        effort: Option<String>,
    },
    ToolStarted {
        id: String,
        name: String,
        description: Option<String>,
    },
    ToolCompleted {
        id: String,
        name: String,
    },
    ToolFailed {
        id: String,
        name: String,
        error: Option<String>,
    },
    /// Coalesced `thinking_delta` / `thinking_end` text.
    Thought(String),
    /// Coalesced `text_delta` / `message_update` assistant text.
    Message(String),
    /// Explicit phase status (turn/model start).
    Status(String),
    Note(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TurnResult {
    pub subtype: String,
    pub session_id: Option<String>,
    pub stop_reason: Option<String>,
    pub final_text: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsedLine {
    Progress(ProgressEvent),
    Result(TurnResult),
    Ignored,
}

#[derive(Debug, Deserialize)]
struct WireLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    event: Option<WireEvent>,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default, rename = "stopReason")]
    stop_reason: Option<String>,
    #[serde(default, rename = "finalText")]
    final_text: Option<String>,
    #[serde(default)]
    error: Option<Value>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WireEvent {
    #[serde(rename = "type")]
    kind: Option<String>,
    #[serde(default, rename = "toolCallId")]
    tool_call_id: Option<String>,
    #[serde(default, rename = "toolName")]
    tool_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    arguments: Option<Value>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    delta: Option<String>,
}

pub fn parse_stdout_line(line: &str) -> ParsedLine {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return ParsedLine::Ignored;
    }
    let Ok(wire) = serde_json::from_str::<WireLine>(trimmed) else {
        return ParsedLine::Progress(ProgressEvent::Note(trimmed.to_owned()));
    };
    match wire.kind.as_deref() {
        Some("event") => parse_event(wire.event.unwrap_or_default()),
        Some("result") => ParsedLine::Result(TurnResult {
            subtype: wire.subtype.unwrap_or_else(|| "success".to_owned()),
            session_id: nonempty(wire.session_id),
            stop_reason: wire.stop_reason,
            final_text: wire.final_text.unwrap_or_default(),
            error: error_text(wire.error, wire.message),
        }),
        _ => ParsedLine::Progress(ProgressEvent::Note(trimmed.to_owned())),
    }
}

fn parse_event(event: WireEvent) -> ParsedLine {
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
        // Muse Spark `thinking_end` is a full snapshot, not a delta.
        "thinking_end" => ParsedLine::Ignored,
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

fn event_text(event: &WireEvent) -> Option<String> {
    nonempty(event.text.clone())
        .or_else(|| nonempty(event.delta.clone()))
        .or_else(|| nonempty(event.message.clone()))
}

fn tool_description(event: &WireEvent) -> Option<String> {
    nonempty(event.description.clone())
        .or_else(|| nonempty(event.query.clone()))
        .or_else(|| preview_arg(event.arguments.as_ref()))
}

#[rustfmt::skip]
fn preview_arg(value: Option<&Value>) -> Option<String> {
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

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.is_empty())
}

fn error_text(error: Option<Value>, message: Option<String>) -> Option<String> {
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
