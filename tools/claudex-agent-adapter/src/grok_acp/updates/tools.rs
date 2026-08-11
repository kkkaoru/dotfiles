//! ACP ToolCall / Plan → Claude Code display helpers (shared Grok + Copilot).

use agent_client_protocol::{self as acp};
use serde_json::json;
#[cfg(test)]
use serde_json::Value;

use crate::app_server::events::ThreadEventDispatcher;

mod args;
use args::build_tool_input;
#[cfg(test)]
use args::{combine_output, enrich_arguments};

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

mod params;
use params::{
    completed_web_operation, status_only_params, tool_update_params, update_to_tool_call,
};

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
