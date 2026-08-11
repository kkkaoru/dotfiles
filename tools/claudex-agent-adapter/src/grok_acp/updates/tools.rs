//! ACP ToolCall / Plan → Claude Code display helpers (shared Grok + Copilot).

use agent_client_protocol::{self as acp};
use serde_json::{Value, json};

use crate::app_server::events::ThreadEventDispatcher;

mod args;
use args::{build_tool_input, combine_output, enrich_arguments};

use super::web_evidence::{ProviderWebEvidence, completion_evidence, web_operation};
use super::{PROVIDER_TOOL_CALL, PROVIDER_TOOL_UPDATE, dispatch_status};
use super::tools_labels::{tool_display_name, tool_kind_label, tool_status_label};
#[cfg(test)]
use super::tools_labels::tool_content_text;
#[cfg(test)]
use super::tools_labels::tool_kind_name;

#[cfg(test)]
pub(super) fn dispatch_provider_tool_call(
    events: &ThreadEventDispatcher,
    session_id: &str,
    call: acp::ToolCall,
) {
    dispatch_provider_tool_call_with_evidence(events, None, session_id, call);
}

pub(super) fn dispatch_provider_tool_call_with_evidence(
    events: &ThreadEventDispatcher,
    evidence: Option<&ProviderWebEvidence>,
    session_id: &str,
    call: acp::ToolCall,
) {
    let call_id = call.tool_call_id.0.to_string();
    let name = tool_display_name(&call);
    let input = build_tool_input(&call);
    let operation = web_operation(&call.title, Some(call.kind), call.raw_input.as_ref());
    let completed_operation = evidence.and_then(|tracker| {
        operation.as_ref().and_then(|operation| {
            tracker.record(session_id, &call_id, operation.clone());
            (call.status == acp::ToolCallStatus::Completed)
                .then(|| tracker.completion_candidate(session_id, &call_id, None))
                .flatten()
        })
    });
    let mut params = json!({
        "threadId": session_id,
        "callId": call_id,
        "tool": name,
        "title": call.title,
        "kind": tool_kind_label(call.kind),
        "status": tool_status_label(call.status),
        "arguments": input
    });
    if let Some(operation) = completed_operation
        && let Some(metadata) = completion_evidence(operation, call.raw_output, Some(&call.content))
    {
        params["evidence"] = metadata;
        if let Some(tracker) = evidence {
            tracker.mark_completed(session_id, &call_id);
        }
    }
    events.dispatch(json!({
        "method": PROVIDER_TOOL_CALL,
        "params": params
    }));
}

#[cfg(test)]
pub(super) fn dispatch_provider_tool_update(
    events: &ThreadEventDispatcher,
    session_id: &str,
    update: acp::ToolCallUpdate,
) {
    dispatch_provider_tool_update_with_evidence(events, None, session_id, update);
}

pub(super) fn dispatch_provider_tool_update_with_evidence(
    events: &ThreadEventDispatcher,
    evidence: Option<&ProviderWebEvidence>,
    session_id: &str,
    update: acp::ToolCallUpdate,
) {
    let call_id = update.tool_call_id.0.to_string();
    let fields = update.fields;
    let completed_evidence = evidence.and_then(|tracker| {
        completed_web_operation(tracker, session_id, &call_id, &fields).and_then(|operation| {
            let metadata = completion_evidence(
                operation,
                fields.raw_output.clone(),
                fields.content.as_ref(),
            )?;
            tracker
                .mark_completed(session_id, &call_id)
                .then_some(metadata)
        })
    });
    if let Some(call) = update_to_tool_call(&call_id, fields.clone()) {
        dispatch_provider_tool_call_with_evidence(events, evidence, session_id, call);
    } else if let Some(params) = status_only_params(session_id, &call_id, &fields) {
        events.dispatch(json!({ "method": PROVIDER_TOOL_UPDATE, "params": params }));
    } else if let Some(status) = fields.status {
        events.dispatch(json!({
            "method": PROVIDER_TOOL_UPDATE,
            "params": tool_update_params(session_id, &call_id, fields, status, completed_evidence)
        }));
    } else {
        // Content-only patches without a status do not change Claude Code's WIP
        // surface; skip them to cut event spam from chatty ACP providers.
    }
}

fn completed_web_operation(
    evidence: &ProviderWebEvidence,
    session_id: &str,
    call_id: &str,
    fields: &acp::ToolCallUpdateFields,
) -> Option<super::web_evidence::WebOperation> {
    (fields.status == Some(acp::ToolCallStatus::Completed)).then_some(())?;
    let direct_operation = fields
        .title
        .as_deref()
        .and_then(|title| web_operation(title, fields.kind, fields.raw_input.as_ref()));
    evidence.completion_candidate(session_id, call_id, direct_operation)
}

fn update_to_tool_call(call_id: &str, fields: acp::ToolCallUpdateFields) -> Option<acp::ToolCall> {
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

fn status_only_params(
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

fn tool_update_params(
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

pub(super) fn dispatch_plan(events: &ThreadEventDispatcher, session_id: &str, plan: acp::Plan) {
    if plan.entries.is_empty() {
        return;
    }
    // Compact one-line summary instead of reprinting full checklists every tick.
    let total = plan.entries.len();
    let completed = plan
        .entries
        .iter()
        .filter(|entry| matches!(entry.status, acp::PlanEntryStatus::Completed))
        .count();
    let active = plan.entries.iter().find_map(|entry| {
        matches!(entry.status, acp::PlanEntryStatus::InProgress)
            .then(|| entry.content.trim().to_owned())
    });
    let text = match active {
        Some(step) if !step.is_empty() => {
            format!("\nPlan {completed}/{total}: {step}\n")
        }
        _ => format!("\nPlan {completed}/{total}\n"),
    };
    dispatch_status(events, session_id, text);
}


#[cfg(test)]
include!("tools_tests.rs");
