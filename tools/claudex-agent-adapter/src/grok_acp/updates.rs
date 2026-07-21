use std::time::Duration;

use agent_client_protocol::{self as acp};
use serde_json::{Value, json};

use crate::app_server::events::ThreadEventDispatcher;

// Grok ACP owns tool execution. Claude Code is display-only for those tools:
// - AgentThoughtChunk → reasoning (Claude "Thought for …" panel)
// - ToolCall / ToolCallUpdate → item/providerTool/* → Anthropic tool_use SSE
//   (native Claude Code tool cards with expandable input). These are not
//   external Claude tools: SegmentBuilder must not wait for tool_result.
// - SubAgent / retry → short agentMessage status lines (not full tool cards)

const AGENT_MESSAGE_METHOD: &str = "item/agentMessage/delta";
const REASONING_METHOD: &str = "item/reasoning/summaryTextDelta";
const PROVIDER_TOOL_CALL: &str = "item/providerTool/call";
const PROVIDER_TOOL_UPDATE: &str = "item/providerTool/update";

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
            dispatch_provider_tool_call(events, &session_id, call);
        }
        acp::SessionUpdate::ToolCallUpdate(update) => {
            dispatch_provider_tool_update(events, &session_id, update);
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

fn dispatch_provider_tool_call(
    events: &ThreadEventDispatcher,
    session_id: &str,
    call: acp::ToolCall,
) {
    let call_id = call.tool_call_id.0.to_string();
    let name = tool_display_name(&call);
    let input = call
        .raw_input
        .unwrap_or_else(|| json!({"description": call.title}));
    events.dispatch(json!({
        "method": PROVIDER_TOOL_CALL,
        "params": {
            "threadId": session_id,
            "callId": call_id,
            "tool": name,
            "title": call.title,
            "arguments": input
        }
    }));
}

fn dispatch_provider_tool_update(
    events: &ThreadEventDispatcher,
    session_id: &str,
    update: acp::ToolCallUpdate,
) {
    let Some(status) = update.fields.status else {
        return;
    };
    let status = match status {
        acp::ToolCallStatus::Completed => "completed",
        acp::ToolCallStatus::Failed => "failed",
        acp::ToolCallStatus::InProgress => "in_progress",
        acp::ToolCallStatus::Pending => "pending",
        _ => return,
    };
    let call_id = update.tool_call_id.0.to_string();
    let mut params = json!({
        "threadId": session_id,
        "callId": call_id,
        "status": status
    });
    if let Some(title) = update.fields.title {
        params["title"] = json!(title);
    }
    if let Some(raw_input) = update.fields.raw_input {
        params["arguments"] = raw_input;
    }
    if let Some(raw_output) = update.fields.raw_output {
        params["output"] = raw_output;
    }
    events.dispatch(json!({
        "method": PROVIDER_TOOL_UPDATE,
        "params": params
    }));
}

fn tool_display_name(call: &acp::ToolCall) -> String {
    match call.kind {
        acp::ToolKind::Read => return "Read".into(),
        acp::ToolKind::Edit => return "Edit".into(),
        acp::ToolKind::Execute => return "Bash".into(),
        acp::ToolKind::Search => return "Search".into(),
        acp::ToolKind::Fetch => return "WebFetch".into(),
        acp::ToolKind::Delete => return "Delete".into(),
        acp::ToolKind::Move => return "Move".into(),
        acp::ToolKind::Think => return "Think".into(),
        _ => {}
    }
    let title = call.title.trim();
    let stripped = title
        .strip_prefix("Using ")
        .unwrap_or(title)
        .trim_end_matches('…')
        .trim_end_matches("...")
        .trim();
    if stripped.is_empty() {
        "Tool".into()
    } else {
        stripped.to_owned()
    }
}

fn dispatch_status(events: &ThreadEventDispatcher, session_id: &str, delta: String) {
    dispatch_delta(events, session_id, AGENT_MESSAGE_METHOD, &delta);
}

fn dispatch_delta(events: &ThreadEventDispatcher, session_id: &str, method: &str, delta: &str) {
    if delta.is_empty() {
        return;
    }
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
