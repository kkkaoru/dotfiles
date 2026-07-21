use std::time::Duration;

use agent_client_protocol::{self as acp};
use serde_json::{Value, json};

use crate::app_server::events::ThreadEventDispatcher;

// Visible progress must use agentMessage deltas, not reasoning. Once Claude Code
// has opened a text block, SegmentBuilder drops later thinking_delta frames
// (has_visible_output). Routing tool/SubAgent status into reasoning therefore
// makes long Grok tool runs look frozen after the first text token — the main
// UX regression for ClaudeX + grok-acp versus native Claude Code / Grok Build.

const AGENT_MESSAGE_METHOD: &str = "item/agentMessage/delta";
const REASONING_METHOD: &str = "item/reasoning/summaryTextDelta";

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
    notification: acp::SessionNotification,
) {
    let session_id = notification.session_id.0;
    match notification.update {
        acp::SessionUpdate::AgentMessageChunk(chunk) => {
            dispatch_text(events, &session_id, chunk, AGENT_MESSAGE_METHOD);
        }
        acp::SessionUpdate::AgentThoughtChunk(chunk) => {
            dispatch_text(events, &session_id, chunk, REASONING_METHOD);
        }
        acp::SessionUpdate::ToolCall(call) => {
            dispatch_status(events, &session_id, format!("\n\nUsing {}…\n", call.title));
        }
        acp::SessionUpdate::ToolCallUpdate(update) => {
            dispatch_tool_update(events, &session_id, update);
        }
        _ => {}
    }
}

pub(super) fn dispatch_extension(
    events: &ThreadEventDispatcher,
    notification: acp::ExtNotification,
) {
    if notification.method.as_ref() != "_x.ai/session/update" {
        return;
    }
    let params = serde_json::from_str::<Value>(notification.params.get())
        .expect("ACP extension params are validated JSON");
    dispatch_extension_value(events, &params);
}

fn dispatch_extension_value(events: &ThreadEventDispatcher, params: &Value) {
    let Some(session_id) = params.get("sessionId").and_then(Value::as_str) else {
        return;
    };
    let Some(update) = params.get("update") else {
        return;
    };
    match update.get("sessionUpdate").and_then(Value::as_str) {
        Some("subagent_spawned") => dispatch_subagent_started(events, session_id, update),
        Some("subagent_finished") => dispatch_subagent_finished(events, session_id, update),
        Some("retry_state") => dispatch_retry(events, session_id, update),
        Some("turn_completed") => dispatch_usage(events, session_id, update),
        _ => {}
    }
}

fn dispatch_text(
    events: &ThreadEventDispatcher,
    session_id: &str,
    chunk: acp::ContentChunk,
    method: &str,
) {
    if let acp::ContentBlock::Text(text) = chunk.content {
        dispatch_delta(events, session_id, method, &text.text);
    }
}

fn dispatch_status(events: &ThreadEventDispatcher, session_id: &str, delta: String) {
    dispatch_delta(events, session_id, AGENT_MESSAGE_METHOD, &delta);
}

fn dispatch_delta(events: &ThreadEventDispatcher, session_id: &str, method: &str, delta: &str) {
    if delta.is_empty() {
        return;
    }
    // itemId/summaryIndex are only consumed for reasoning; agentMessage path
    // reads params.delta. Keep both so one helper covers both methods.
    events.dispatch(json!({
        "method":method,
        "params":{
            "threadId":session_id,
            "itemId":format!("{session_id}:status"),
            "summaryIndex":0,
            "delta":delta
        }
    }));
}

fn dispatch_tool_update(
    events: &ThreadEventDispatcher,
    session_id: &str,
    update: acp::ToolCallUpdate,
) {
    let Some(status) = update.fields.status else {
        return;
    };
    let Some(title) = update.fields.title else {
        return;
    };
    let marker = match status {
        acp::ToolCallStatus::Completed => "Completed",
        acp::ToolCallStatus::Failed => "Failed",
        acp::ToolCallStatus::InProgress => "Running",
        acp::ToolCallStatus::Pending => "Pending",
        // ToolCallStatus is non-exhaustive across ACP schema versions.
        _ => return,
    };
    dispatch_status(events, session_id, format!("{marker}: {title}\n"));
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
