use serde::Deserialize;
use serde_json::Value;

mod chrome;
mod progress;
use chrome::has_status_prefix;
pub use chrome::is_canned_progress;
pub use progress::{progress_to_updates, result_is_error, result_message, turn_cancelled_updates};

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
    /// Coalesced `thinking_delta` text (and rare `thinking_end` snapshots when
    /// Muse skipped deltas for that unit).
    Thought(String),
    /// Muse Spark full thinking snapshot. Coalescer keeps it only when no
    /// `thinking_delta` has been emitted for the current unit (avoids the
    /// Thought-for flicker from replaying the whole buffer after deltas).
    ThoughtEnd(String),
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
pub(super) struct WireLine {
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
pub(super) struct WireEvent {
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

mod parse;
use parse::{error_text, nonempty, parse_event};
