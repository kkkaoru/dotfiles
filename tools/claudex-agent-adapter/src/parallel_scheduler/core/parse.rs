use std::collections::HashSet;

use serde_json::Value;

use super::Workunit;

pub(super) fn legacy_cc_agent_round_ids(message: &Value) -> Option<Vec<String>> {
    let ids = message
        .get("content")?
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter(|block| {
            block
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.starts_with("cc_Agent_") && !name.contains("_batch_"))
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

pub(super) fn record_message_content(
    content: &[Value],
    launched: &mut Vec<Workunit>,
    completed: &mut HashSet<String>,
    exact_async_acknowledgement: bool,
) {
    for block in content {
        match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => parse_subagent_tool_use(block, launched),
            Some("tool_result") => {
                record_completed_tool(block, completed, exact_async_acknowledgement);
            }
            _ => {}
        }
    }
}

fn record_completed_tool(
    block: &Value,
    completed: &mut HashSet<String>,
    exact_async_acknowledgement: bool,
) {
    if exact_async_acknowledgement {
        return;
    }
    let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
        return;
    };
    completed.insert(id.to_owned());
}

pub(super) fn record_task_notification(message: &Value, completed: &mut HashSet<String>) {
    let Some(content) = message.get("content") else {
        return;
    };
    match content {
        Value::String(text) => record_task_notification_text(text, completed),
        Value::Array(items) => {
            for text in items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|item| item.get("text").and_then(Value::as_str))
            {
                record_task_notification_text(text, completed);
            }
        }
        _ => {}
    }
}

fn record_task_notification_text(text: &str, completed: &mut HashSet<String>) {
    let text = text.trim();
    if !text.starts_with("<task-notification>")
        || !["completed", "failed", "stopped"]
            .iter()
            .any(|status| text.contains(&format!("<status>{status}</status>")))
    {
        return;
    }
    if let Some(tool_use_id) = xml_tag(text, "tool-use-id") {
        completed.insert(tool_use_id.to_owned());
    }
}

fn xml_tag<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let value = text.split_once(&open)?.1.split_once(&close)?.0.trim();
    (!value.is_empty()).then_some(value)
}

#[path = "parse_tools.rs"]
mod parse_tools;
pub(super) use parse_tools::parse_subagent_tool_use;
