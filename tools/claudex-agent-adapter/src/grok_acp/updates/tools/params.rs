use agent_client_protocol as acp;
use serde_json::{Value, json};

use super::args::{combine_output, enrich_arguments};
use super::tool_status_label;
use super::super::web_evidence::{ProviderWebEvidence, WebOperation, web_operation};

pub(super) fn completed_web_operation(
    evidence: &ProviderWebEvidence,
    session_id: &str,
    call_id: &str,
    fields: &acp::ToolCallUpdateFields,
) -> Option<WebOperation> {
    (fields.status == Some(acp::ToolCallStatus::Completed)).then_some(())?;
    let direct_operation = fields
        .title
        .as_deref()
        .and_then(|title| web_operation(title, fields.kind, fields.raw_input.as_ref()));
    evidence.completion_candidate(session_id, call_id, direct_operation)
}

pub(super) fn update_to_tool_call(call_id: &str, fields: acp::ToolCallUpdateFields) -> Option<acp::ToolCall> {
    let title = fields.title?;
    let status = fields.status?;
    if !matches!(
        status,
        acp::ToolCallStatus::Pending | acp::ToolCallStatus::InProgress
    ) {
        return None;
    }
    // Cursor and other ACP agents often start tools with only title/status and no rawInput.
    // Still open a WIP card so Claude Code shows progress instead of a silent spinner.
    let raw_input = fields.raw_input.clone().unwrap_or_else(|| json!({}));
    let mut call = acp::ToolCall::new(call_id.to_owned(), title).raw_input(raw_input);
    if let Some(kind) = fields.kind {
        call = call.kind(kind);
    }
    if let Some(content) = fields.content {
        call = call.content(content);
    }
    if let Some(locations) = fields.locations {
        call = call.locations(locations);
    }
    Some(call.status(status))
}

pub(super) fn status_only_params(
    session_id: &str,
    call_id: &str,
    fields: &acp::ToolCallUpdateFields,
) -> Option<Value> {
    let status = fields.status?;
    if matches!(
        status,
        acp::ToolCallStatus::Pending | acp::ToolCallStatus::InProgress
    ) && fields.raw_output.is_none()
        && fields.content.is_none()
    {
        let title = fields
            .title
            .clone()
            .unwrap_or_else(|| "provider tool".to_owned());
        let mut params = json!({
            "threadId": session_id,
            "callId": call_id,
            "status": tool_status_label(status)
        });
        params["title"] = json!(title);
        return Some(params);
    }
    None
}

pub(super) fn tool_update_params(
    session_id: &str,
    call_id: &str,
    fields: acp::ToolCallUpdateFields,
    status: acp::ToolCallStatus,
    evidence: Option<Value>,
) -> Value {
    let mut params = json!({
        "threadId": session_id,
        "callId": call_id,
        "status": tool_status_label(status),
    });
    if let Some(title) = fields.title {
        params["title"] = json!(title);
    }
    if let Some(raw_input) = fields.raw_input {
        params["arguments"] = enrich_arguments(raw_input, &fields.content, &fields.locations);
    }
    if let Some(output) = combine_output(fields.raw_output, fields.content.as_ref()) {
        params["output"] = output;
    }
    if let Some(evidence) = evidence {
        params["evidence"] = evidence;
    }
    params
}
