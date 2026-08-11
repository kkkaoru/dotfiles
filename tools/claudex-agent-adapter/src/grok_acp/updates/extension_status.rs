use std::time::Duration;

use serde_json::{Value, json};

use crate::app_server::events::ThreadEventDispatcher;

use super::AGENT_MESSAGE_METHOD;

/// Progress status as agent-message deltas with `itemId` `...:status`.
/// The stream builder commits these as visible assistant text so SubAgent UIs
/// show native ACP work without inventing executable `tool_use` cards.
pub(in crate::grok_acp) fn dispatch_status(events: &ThreadEventDispatcher, session_id: &str, delta: String) {
    dispatch_delta(
        events,
        session_id,
        AGENT_MESSAGE_METHOD,
        &format!("{session_id}:status"),
        0,
        &delta,
    );
}

pub(super) fn dispatch_delta(
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

pub(super) fn dispatch_subagent_started(events: &ThreadEventDispatcher, session_id: &str, update: &Value) {
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

pub(super) fn dispatch_subagent_finished(events: &ThreadEventDispatcher, session_id: &str, update: &Value) {
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

pub(super) fn dispatch_retry(events: &ThreadEventDispatcher, session_id: &str, update: &Value) {
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

pub(super) fn dispatch_usage(events: &ThreadEventDispatcher, session_id: &str, update: &Value) {
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

pub(super) fn string_field<'a>(value: &'a Value, field: &str, fallback: &'a str) -> &'a str {
    value.get(field).and_then(Value::as_str).unwrap_or(fallback)
}
