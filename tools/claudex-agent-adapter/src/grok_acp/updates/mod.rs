//! Shared ACP → Claude Code bridge (Grok ACP and Copilot ACP both use this).
//!
//! | ACP update | Claude Code surface |
//! |---|---|
//! | AgentThoughtChunk | thinking units (`thinking_delta`, one block per unit) |
//! | AgentMessageChunk | assistant text |
//! | ToolCall / ToolCallUpdate | visible progress text (never executable `tool_use`) |
//! | Plan | compact plan status text (debounced) |
//! | SessionInfo / mode | ignored (noisy vs native Claude Code) |
//! | xAI SubAgent extensions | compact status |

mod thought_units;
mod tools;
mod tools_labels;
mod web_evidence;
mod extension_status;

use agent_client_protocol::{self as acp};
use serde_json::{Value, json};

use crate::app_server::events::ThreadEventDispatcher;

pub(crate) use thought_units::ThoughtUnits;
use extension_status::{
    dispatch_delta, dispatch_retry, dispatch_subagent_finished, dispatch_subagent_started,
    dispatch_usage,
};
pub(in crate::grok_acp) use extension_status::dispatch_status;
use tools::{
    dispatch_plan, dispatch_provider_tool_call_with_evidence,
    dispatch_provider_tool_update_with_evidence,
};

pub(super) const AGENT_MESSAGE_METHOD: &str = "item/agentMessage/delta";
pub(super) const REASONING_METHOD: &str = "item/reasoning/summaryTextDelta";
pub(super) const PROVIDER_TOOL_CALL: &str = "item/providerTool/call";
pub(super) const PROVIDER_TOOL_UPDATE: &str = "item/providerTool/update";

pub(super) fn dispatch_error(events: &ThreadEventDispatcher, session_id: &str, message: String) {
    events.dispatch(json!({
        "method":"error",
        "params":{
            "threadId":session_id,
            "willRetry":false,
            "error":{"message":message}
        }
    }));
}

pub(super) fn dispatch_notification(
    events: &ThreadEventDispatcher,
    thoughts: &ThoughtUnits,
    notification: acp::SessionNotification,
) {
    let session_id = notification.session_id.0;
    match notification.update {
        acp::SessionUpdate::AgentMessageChunk(chunk) => {
            dispatch_message(events, &session_id, chunk);
        }
        acp::SessionUpdate::AgentThoughtChunk(chunk) => {
            dispatch_thought(events, thoughts, &session_id, chunk);
        }
        acp::SessionUpdate::ToolCall(call) => {
            // Only tool *starts* open a new thought unit. Per-update breaks made
            // summaryIndex thrash and reordered thinking chunks in the UI log.
            thoughts.break_after_interrupt(&session_id);
            dispatch_provider_tool_call_with_evidence(
                events,
                Some(&thoughts.provider_web_evidence),
                &session_id,
                call,
            );
        }
        acp::SessionUpdate::ToolCallUpdate(update) => {
            dispatch_provider_tool_update_with_evidence(
                events,
                Some(&thoughts.provider_web_evidence),
                &session_id,
                update,
            );
        }
        acp::SessionUpdate::Plan(plan) => {
            dispatch_plan(events, &session_id, plan);
        }
        // Mode/title chatter is far noisier than Claude Code's own session UI.
        // Drop it so ACP turns do not spam assistant/thinking streams.
        acp::SessionUpdate::CurrentModeUpdate(_)
        | acp::SessionUpdate::SessionInfoUpdate(_)
        | acp::SessionUpdate::UserMessageChunk(_)
        | acp::SessionUpdate::AvailableCommandsUpdate(_)
        | acp::SessionUpdate::ConfigOptionUpdate(_) => {}
        _ => {}
    }
}

pub(super) fn dispatch_extension(
    events: &ThreadEventDispatcher,
    thoughts: &ThoughtUnits,
    notification: acp::ExtNotification,
) {
    if notification.method.as_ref() != "_x.ai/session/update" {
        return;
    }
    let params = serde_json::from_str::<Value>(notification.params.get())
        .expect("ACP extension params are validated JSON");
    dispatch_extension_value(events, thoughts, &params);
}

fn dispatch_extension_value(
    events: &ThreadEventDispatcher,
    thoughts: &ThoughtUnits,
    params: &Value,
) {
    let Some(session_id) = params.get("sessionId").and_then(Value::as_str) else {
        return;
    };
    let Some(update) = params.get("update") else {
        return;
    };
    match update.get("sessionUpdate").and_then(Value::as_str) {
        Some("subagent_spawned") => {
            thoughts.break_after_interrupt(session_id);
            dispatch_subagent_started(events, session_id, update);
        }
        Some("subagent_finished") => {
            thoughts.break_after_interrupt(session_id);
            dispatch_subagent_finished(events, session_id, update);
        }
        Some("retry_state") => {
            thoughts.break_after_interrupt(session_id);
            dispatch_retry(events, session_id, update);
        }
        Some("turn_completed") => {
            thoughts.clear(session_id);
            dispatch_usage(events, session_id, update);
        }
        _ => {}
    }
}

fn dispatch_message(events: &ThreadEventDispatcher, session_id: &str, chunk: acp::ContentChunk) {
    if let acp::ContentBlock::Text(text) = chunk.content {
        dispatch_delta(
            events,
            session_id,
            AGENT_MESSAGE_METHOD,
            &format!("{session_id}:message"),
            0,
            &text.text,
        );
    }
}

fn dispatch_thought(
    events: &ThreadEventDispatcher,
    thoughts: &ThoughtUnits,
    session_id: &str,
    chunk: acp::ContentChunk,
) {
    let acp::ContentBlock::Text(text) = chunk.content else {
        return;
    };
    if text.text.trim().is_empty() {
        return;
    }
    let item_id = format!("{session_id}:reasoning");
    for (summary_index, piece) in thoughts.partition(session_id, &text.text) {
        dispatch_delta(
            events,
            session_id,
            REASONING_METHOD,
            &item_id,
            summary_index,
            &piece,
        );
    }
}


#[cfg(test)]
mod tests;
