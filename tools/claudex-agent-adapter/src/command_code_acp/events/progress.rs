use agent_client_protocol as acp;
use serde_json::json;

use super::chrome::{has_status_prefix, native_message, strip_canned_progress, thought_chunk};
use super::{ProgressEvent, TurnResult};
use crate::command_code_acp::tool_chrome::{tool_kind, tool_raw_input};

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
        ProgressEvent::Thought(text) | ProgressEvent::Status(text) => thought_updates(text),
        ProgressEvent::ThoughtEnd(text) => {
            // Coalescer normally collapses ThoughtEnd → Thought; keep a fallback.
            thought_updates(text)
        }
        ProgressEvent::Message(text) => message_updates(text),
        ProgressEvent::Note(_) => Vec::new(),
    }
}

fn thought_updates(text: &str) -> Vec<acp::SessionUpdate> {
    strip_canned_progress(text)
        .map(|text| thought_chunk(&text))
        .into_iter()
        .collect()
}

fn message_updates(text: &str) -> Vec<acp::SessionUpdate> {
    let Some(text) = strip_canned_progress(text) else {
        return Vec::new();
    };
    if has_status_prefix(text.trim()) {
        vec![thought_chunk(&text)]
    } else {
        vec![native_message(&text)]
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
