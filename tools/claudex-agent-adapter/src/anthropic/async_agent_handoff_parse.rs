use std::collections::HashSet;

use serde_json::Value;

use super::{ASYNC_LAUNCH_PREFIX, BACKGROUND_MARKER, MessagesRequest};

pub(super) fn pending_tools_outside_async_launches(
    pending_ids: &[String],
    async_launch_ids: &[String],
) -> bool {
    let async_set = async_launch_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    pending_ids
        .iter()
        .any(|pending_id| !async_set.contains(pending_id.as_str()))
}

/// Collect successful async background-launch tool_result IDs from a user
/// message. Non-result blocks and ordinary tool results are ignored so a mixed
/// Claude Code continuation can still hand control back once every Agent/Task
/// in the latest round is acknowledged as backgrounded.
pub(super) fn async_launch_tool_results(message: &Value) -> Option<Vec<String>> {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let blocks = message.get("content")?.as_array()?;
    if blocks.is_empty() {
        return None;
    }
    let mut ids = Vec::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) != Some("tool_result")
            || block.get("is_error").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(text) = block.get("content").and_then(strict_result_text) else {
            continue;
        };
        if !text.trim_start().starts_with(ASYNC_LAUNCH_PREFIX) || !text.contains(BACKGROUND_MARKER)
        {
            continue;
        }
        let Some(id) = block
            .get("tool_use_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
        else {
            continue;
        };
        ids.push(id);
    }
    (!ids.is_empty()).then_some(ids)
}

/// Return the successful async launch IDs only when they are a duplicate-free,
/// exact match for the expected tool round. Both handoff and the
/// scheduler use this predicate so a partial or replayed acknowledgement cannot
/// make the two lifecycle views diverge.
pub(crate) fn exact_async_launch_acknowledgement(
    message: &Value,
    expected_tool_use_ids: &[String],
) -> Option<Vec<String>> {
    let result_ids = async_launch_tool_results(message)?;
    if result_ids.len() != expected_tool_use_ids.len() || result_ids.is_empty() {
        return None;
    }
    let result_set = result_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let expected_set = expected_tool_use_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if result_set.len() != result_ids.len()
        || expected_set.len() != expected_tool_use_ids.len()
        || result_set != expected_set
    {
        return None;
    }
    Some(result_ids)
}

pub(super) fn strict_result_text(content: &Value) -> Option<String> {
    match content {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) if !items.is_empty() => {
            let mut text = String::new();
            for item in items {
                append_strict_result_text(&mut text, item)?;
            }
            Some(text)
        }
        _ => None,
    }
}

pub(super) fn append_strict_result_text(text: &mut String, item: &Value) -> Option<()> {
    (item.get("type").and_then(Value::as_str) == Some("text")).then_some(())?;
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(item.get("text").and_then(Value::as_str)?);
    Some(())
}

pub(super) fn latest_agent_tool_round_ids(request: &MessagesRequest) -> Option<Vec<String>> {
    request
        .messages
        .iter()
        .rev()
        .skip(1)
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .find_map(agent_tool_round_ids)
}

pub(crate) fn agent_tool_round_ids(message: &Value) -> Option<Vec<String>> {
    let ids = message
        .get("content")?
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter(|block| {
            block
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(crate::anthropic::agent_effort::is_agent_tool)
        })
        .map(|block| {
            block
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    (!ids.is_empty()).then_some(ids)
}
