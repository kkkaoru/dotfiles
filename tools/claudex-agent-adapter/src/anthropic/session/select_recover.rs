use std::collections::HashSet;

use serde_json::{Value, json};

use crate::anthropic::content::{ToolResult, remember_consumed_tool_id};
use crate::anthropic::{MessagesRequest, Session};

const UNKNOWN_TOOL_USE_ID: &str = "unknown";
const UNKNOWN_TOOL_NAME: &str = "tool";

pub(in crate::anthropic::session) fn maybe_sanitize_recovered_request(
    recovered: bool,
    request: &MessagesRequest,
) -> Option<MessagesRequest> {
    if !recovered || !any_incomplete_tool_use(&request.messages) {
        return None;
    }
    tracing::warn!(
        incomplete_tool_use = incomplete_tool_use_count(&request.messages),
        "dropping incomplete tool_use after adapter session loss"
    );
    let mut request = request.clone();
    request.messages = sanitize_session_loss_replay(&request.messages);
    Some(request)
}

pub(super) async fn remember_recovered_tool_results(session: &Session, results: &[ToolResult]) {
    let mut consumed = session.consumed_tool_ids.lock().await;
    results.iter().for_each(|result| {
        remember_consumed_tool_id(&mut consumed, result.tool_use_id.clone());
    });
}

pub(super) fn incomplete_tool_use_ids(messages: &[Value]) -> HashSet<String> {
    incomplete_tool_use_blocks(messages)
        .filter_map(|block| block.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

pub(super) fn incomplete_tool_use_count(messages: &[Value]) -> usize {
    incomplete_tool_use_blocks(messages).count()
}

pub(super) fn any_incomplete_tool_use(messages: &[Value]) -> bool {
    incomplete_tool_use_blocks(messages).next().is_some()
}

fn incomplete_tool_use_blocks(messages: &[Value]) -> impl Iterator<Item = &Value> {
    messages
        .iter()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter(|block| !tool_use_input_is_complete(block.get("input").unwrap_or(&Value::Null)))
}

pub(super) fn sanitize_session_loss_replay(messages: &[Value]) -> Vec<Value> {
    rewrite_incomplete_tool_uses(messages, &incomplete_tool_use_ids(messages))
}

pub(super) fn tool_use_input_is_complete(input: &Value) -> bool {
    match input {
        Value::Object(map) => !map.is_empty(),
        Value::String(raw) => complete_object_json(raw),
        _ => false,
    }
}

fn complete_object_json(raw: &str) -> bool {
    serde_json::from_str::<Value>(raw)
        .ok()
        .is_some_and(|parsed| parsed.as_object().is_some_and(|map| !map.is_empty()))
}

fn rewrite_incomplete_tool_uses(
    messages: &[Value],
    incomplete_ids: &HashSet<String>,
) -> Vec<Value> {
    messages
        .iter()
        .map(|message| rewrite_message(message, incomplete_ids))
        .collect()
}

fn rewrite_message(message: &Value, incomplete_ids: &HashSet<String>) -> Value {
    let Some(blocks) = message.get("content").and_then(Value::as_array) else {
        return message.clone();
    };
    let mut rewritten = message.clone();
    rewritten["content"] = Value::Array(
        blocks
            .iter()
            .map(|block| rewrite_block(block, incomplete_ids))
            .collect(),
    );
    rewritten
}

fn rewrite_block(block: &Value, incomplete_ids: &HashSet<String>) -> Value {
    match block.get("type").and_then(Value::as_str) {
        Some("tool_use") => rewrite_tool_use(block),
        Some("tool_result") => rewrite_tool_result(block, incomplete_ids),
        _ => block.clone(),
    }
}

fn rewrite_tool_use(block: &Value) -> Value {
    if tool_use_input_is_complete(block.get("input").unwrap_or(&Value::Null)) {
        return block.clone();
    }
    let id = block
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(UNKNOWN_TOOL_USE_ID);
    let name = block
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(UNKNOWN_TOOL_NAME);
    json!({
        "type":"text",
        "text":format!(
            "Adapter session was lost before tool_use `{id}` ({name}) received complete JSON arguments. The incomplete call was not replayed."
        )
    })
}

fn rewrite_tool_result(block: &Value, incomplete_ids: &HashSet<String>) -> Value {
    let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
        return block.clone();
    };
    if !incomplete_ids.contains(id) {
        return block.clone();
    }
    json!({
        "type":"text",
        "text":format!(
            "Adapter session was lost before tool_use `{id}` received complete JSON arguments. The in-flight tool was failed instead of replaying empty or truncated input."
        )
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "select_recover_tests.rs"]
mod tests;
