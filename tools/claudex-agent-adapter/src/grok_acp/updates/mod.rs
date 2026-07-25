//! Shared ACP → Claude Code bridge (Grok ACP and Copilot ACP both use this).
//!
//! | ACP update | Claude Code surface |
//! |---|---|
//! | AgentThoughtChunk | thinking units (`thinking_delta`, one block per unit) |
//! | AgentMessageChunk | assistant text |
//! | ToolCall / ToolCallUpdate | ephemeral WIP progress (never executable `tool_use`) |
//! | Plan | compact plan status (debounced; not answer text) |
//! | SessionInfo / mode | ignored (noisy vs native Claude Code) |
//! | xAI SubAgent extensions | compact status |

mod thought_units;
mod tools;

use std::time::Duration;

use agent_client_protocol::{self as acp};
use serde_json::{Value, json};

use crate::app_server::events::ThreadEventDispatcher;

pub(crate) use thought_units::ThoughtUnits;
use tools::{dispatch_plan, dispatch_provider_tool_call, dispatch_provider_tool_update};

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
            thoughts.break_after_interrupt(&session_id);
            dispatch_provider_tool_call(events, &session_id, call);
        }
        acp::SessionUpdate::ToolCallUpdate(update) => {
            thoughts.break_after_interrupt(&session_id);
            dispatch_provider_tool_update(events, &session_id, update);
        }
        acp::SessionUpdate::Plan(plan) => {
            thoughts.break_after_interrupt(&session_id);
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

/// Status that must not become answer text. Uses the shared agent-message path
/// with a dedicated `itemId` suffix so the stream builder can treat it as
/// ephemeral WIP and strip it from the committed transcript.
pub(super) fn dispatch_status(events: &ThreadEventDispatcher, session_id: &str, delta: String) {
    dispatch_delta(
        events,
        session_id,
        AGENT_MESSAGE_METHOD,
        &format!("{session_id}:status"),
        0,
        &delta,
    );
}

fn dispatch_delta(
    events: &ThreadEventDispatcher,
    session_id: &str,
    method: &str,
    item_id: &str,
    summary_index: i64,
    delta: &str,
) {
    if delta.is_empty() {
        return;
    }
    events.dispatch(json!({
        "method":method,
        "params":{
            "threadId":session_id,
            "itemId":item_id,
            "summaryIndex":summary_index,
            "delta":delta
        }
    }));
}

fn dispatch_subagent_started(events: &ThreadEventDispatcher, session_id: &str, update: &Value) {
    let description = string_field(update, "description", "SubAgent");
    let model = string_field(update, "model", "unknown model");
    let effort = update
        .get("reasoning_effort")
        .and_then(Value::as_str)
        .map_or_else(String::new, |value| format!(", {value} effort"));
    dispatch_status(
        events,
        session_id,
        format!("\n\nSubAgent started: {description} ({model}{effort})\n"),
    );
}

fn dispatch_subagent_finished(events: &ThreadEventDispatcher, session_id: &str, update: &Value) {
    let status = string_field(update, "status", "finished");
    let duration = update
        .get("duration_ms")
        .and_then(Value::as_u64)
        .map(Duration::from_millis)
        .map_or_else(String::new, |value| {
            format!(" in {:.1}s", value.as_secs_f64())
        });
    dispatch_status(events, session_id, format!("SubAgent {status}{duration}\n"));
}

fn dispatch_retry(events: &ThreadEventDispatcher, session_id: &str, update: &Value) {
    let attempt = update.get("attempt").and_then(Value::as_u64).unwrap_or(1);
    let max = update
        .get("max_retries")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    dispatch_status(
        events,
        session_id,
        format!("Retrying provider request ({attempt}/{max})…\n"),
    );
}

fn dispatch_usage(events: &ThreadEventDispatcher, session_id: &str, update: &Value) {
    let Some(usage) = update.get("usage") else {
        return;
    };
    events.dispatch(json!({
        "method":"thread/tokenUsage/updated",
        "params":{
            "threadId":session_id,
            "tokenUsage":{"last":{
                "inputTokens":usage.get("inputTokens").and_then(Value::as_u64).unwrap_or(0),
                "outputTokens":usage.get("outputTokens").and_then(Value::as_u64).unwrap_or(0),
                "reasoningOutputTokens":usage.get("reasoningTokens")
                    .and_then(Value::as_u64).unwrap_or(0)
            }}
        }
    }));
}

fn string_field<'a>(value: &'a Value, field: &str, fallback: &'a str) -> &'a str {
    value.get(field).and_then(Value::as_str).unwrap_or(fallback)
}

#[cfg(test)]
mod tests;
