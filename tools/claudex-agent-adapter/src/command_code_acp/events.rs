use agent_client_protocol as acp;
use serde::Deserialize;
use serde_json::{Value, json};

use super::tool_chrome::{tool_kind, tool_raw_input};

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

pub fn progress_to_updates(event: &ProgressEvent) -> Vec<acp::SessionUpdate> {
    match event {
        ProgressEvent::Started { .. } => Vec::new(),
        ProgressEvent::ToolStarted {
            id,
            name,
            description,
        } => vec![acp::SessionUpdate::ToolCall(
            acp::ToolCall::new(id.clone(), name.clone())
                .kind(tool_kind(name))
                .status(acp::ToolCallStatus::InProgress)
                .raw_input(tool_raw_input(name, description.as_deref())),
        )],
        ProgressEvent::ToolCompleted { id, name } => {
            vec![acp::SessionUpdate::ToolCallUpdate(
                acp::ToolCallUpdate::new(
                    id.clone(),
                    acp::ToolCallUpdateFields::new()
                        .status(acp::ToolCallStatus::Completed)
                        .title(name.clone()),
                ),
            )]
        }
        ProgressEvent::ToolFailed { id, name, error } => {
            let mut fields = acp::ToolCallUpdateFields::new()
                .status(acp::ToolCallStatus::Failed)
                .title(name.clone());
            if let Some(detail) = error
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
            {
                fields = fields.raw_output(json!(detail));
            }
            vec![acp::SessionUpdate::ToolCallUpdate(
                acp::ToolCallUpdate::new(id.clone(), fields),
            )]
        }
        ProgressEvent::Thought(text) | ProgressEvent::Status(text) if !is_canned_progress(text) => {
            vec![thought_chunk(text)]
        }
        ProgressEvent::Message(text) if !is_canned_progress(text) => {
            if has_status_prefix(text.trim()) {
                vec![thought_chunk(text)]
            } else {
                vec![native_message(text)]
            }
        }
        ProgressEvent::Thought(_)
        | ProgressEvent::Status(_)
        | ProgressEvent::Message(_)
        | ProgressEvent::Note(_) => Vec::new(),
    }
}

pub fn result_message(result: &TurnResult) -> String {
    if !result.final_text.trim().is_empty() {
        return result.final_text.clone();
    }
    if let Some(error) = &result.error {
        return error.clone();
    }
    if result.subtype == "success" {
        String::new()
    } else {
        format!(
            "Command Code headless ended with subtype `{}`",
            result.subtype
        )
    }
}

pub fn result_is_error(result: &TurnResult) -> bool {
    result.subtype == "error"
        || result.error.is_some()
        || matches!(result.stop_reason.as_deref(), Some("error"))
}

pub fn turn_cancelled_updates() -> Vec<acp::SessionUpdate> {
    vec![native_message("Command Code cancelled")]
}

fn event_text(event: &WireEvent) -> Option<String> {
    nonempty(event.text.clone())
        .or_else(|| nonempty(event.delta.clone()))
        .or_else(|| nonempty(event.message.clone()))
}

fn ensure_trailing_newline(text: &str) -> String {
    format!("{text}\n")
}

fn native_message(text: &str) -> acp::SessionUpdate {
    message(ensure_trailing_newline(text.trim()))
}

fn thought_chunk(text: &str) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(ensure_trailing_newline(text.trim())),
    )))
}

fn is_thought_for_chrome(text: &str) -> bool {
    text.trim().to_ascii_lowercase().starts_with("thought for ")
}

fn is_canned_progress(text: &str) -> bool {
    let t = text.trim().trim_start_matches(['●', '▶', '✓', '✗', ' ']);
    is_thought_for_chrome(t)
        || t.contains("ツール結果待ち")
        || t.contains("続きの調査または回答")
        || t.contains("次: タスク実行")
        || t.contains("次: ツールまたは回答")
        || t.contains("次: トークン待ち")
        || t.contains("次: 別手段または報告")
        || t.contains("次: 中断")
        || t.starts_with("起動: Command Code")
        || (t.starts_with("実行中:") && t.contains("。次:"))
        || (t.starts_with("完了:") && t.contains("。次:"))
        || (t.starts_with("失敗:") && t.contains("。次:"))
        || (t.starts_with("ターン") && t.contains("開始"))
        || t.starts_with("モデル要求中:")
}

fn has_status_prefix(text: &str) -> bool {
    text.starts_with('●') || text.starts_with('▶') || text.starts_with('✓') || text.starts_with('✗')
}

fn message(text: impl Into<String>) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text.into()),
    )))
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
