use agent_client_protocol as acp;
use serde::Deserialize;
use serde_json::Value;

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

#[derive(Debug, Deserialize)]
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
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
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
        Some("event") => parse_event(wire.event.unwrap_or(WireEvent {
            kind: None,
            tool_call_id: None,
            tool_name: None,
            description: None,
            error: None,
            message: None,
        })),
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
                description: nonempty(event.description),
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
        "" => ParsedLine::Ignored,
        other => ParsedLine::Progress(ProgressEvent::Note(match nonempty(event.message) {
            Some(message) => format!("{other}: {message}"),
            None => other.to_owned(),
        })),
    }
}

pub fn progress_to_updates(event: &ProgressEvent) -> Vec<acp::SessionUpdate> {
    match event {
        ProgressEvent::Started { model, effort } => {
            let effort = effort
                .as_deref()
                .map(|value| format!(", effort={value}"))
                .unwrap_or_default();
            vec![thought(format!(
                "▶ Command Code headless starting ({model}{effort})\n"
            ))]
        }
        ProgressEvent::ToolStarted {
            id,
            name,
            description,
        } => {
            let title = tool_title(name, description.as_deref());
            vec![
                thought(format!("▶ {title}\n")),
                acp::SessionUpdate::ToolCall(
                    acp::ToolCall::new(id.clone(), title)
                        .kind(tool_kind(name))
                        .status(acp::ToolCallStatus::InProgress),
                ),
            ]
        }
        ProgressEvent::ToolCompleted { id, name } => {
            vec![
                thought(format!("✓ {name}\n")),
                acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                    id.clone(),
                    acp::ToolCallUpdateFields::new()
                        .status(acp::ToolCallStatus::Completed)
                        .title(name.clone()),
                )),
            ]
        }
        ProgressEvent::ToolFailed { id, name, error } => {
            let detail = error
                .as_deref()
                .map(|value| format!(": {value}"))
                .unwrap_or_default();
            vec![
                thought(format!("✗ {name}{detail}\n")),
                acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
                    id.clone(),
                    acp::ToolCallUpdateFields::new()
                        .status(acp::ToolCallStatus::Failed)
                        .title(name.clone()),
                )),
            ]
        }
        ProgressEvent::Note(note) => vec![thought(format!("{note}\n"))],
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

fn thought(text: impl Into<String>) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text.into()),
    )))
}

fn tool_title(name: &str, description: Option<&str>) -> String {
    match description {
        Some(description) if !description.is_empty() => format!("{name}: {description}"),
        _ => name.to_owned(),
    }
}

fn tool_kind(name: &str) -> acp::ToolKind {
    let lower = name.to_ascii_lowercase();
    if lower.contains("read") || lower.contains("grep") || lower.contains("glob") {
        acp::ToolKind::Read
    } else if lower.contains("write") || lower.contains("edit") || lower.contains("patch") {
        acp::ToolKind::Edit
    } else if lower.contains("bash") || lower.contains("shell") || lower.contains("exec") {
        acp::ToolKind::Execute
    } else if lower.contains("search") || lower.contains("web") {
        acp::ToolKind::Search
    } else {
        acp::ToolKind::Other
    }
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
