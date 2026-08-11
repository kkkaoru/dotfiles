use std::collections::HashMap;

use serde_json::Value;

use super::{
    records::LaunchRecord,
    records_scope::{find_recipient, is_launch_result, parse_recipient, xml_value},
    value_text,
};

pub(super) fn mark_failed_launch_result(
    launches: &mut [LaunchRecord],
    contexts: &HashMap<String, (String, Option<String>, Option<String>)>,
    block: &Value,
) {
    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
        return;
    }
    let Some(tool_use_id) = block.get("tool_use_id").and_then(Value::as_str) else {
        return;
    };
    if let Some(launch) = launches
        .iter_mut()
        .find(|launch| launch.key == tool_use_id && launch.status == "pending")
    {
        launch.status = "failed".to_owned();
        return;
    }
    // Only error tool_results retire a resume target. Successful resume prose
    // often lacks the spawn launch phrases, and must not mark the agent failed.
    if !block
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return;
    }
    // Auto-resume failures must retire the target recipient or rewrite keeps
    // reinjecting the same dead agentId on every same-scope follow-up.
    if let Some((_, _, Some(resume))) = contexts.get(tool_use_id) {
        set_recipient_status(launches, resume, "failed".to_owned());
    }
}

pub(super) fn set_task_status(launches: &mut [LaunchRecord], task_id: &str, status: String) {
    if let Some(launch) = launches
        .iter_mut()
        .find(|launch| launch.key == task_id || launch.recipient == task_id)
    {
        launch.status = status;
    }
}

pub(super) fn set_recipient_status(launches: &mut [LaunchRecord], recipient: &str, status: String) {
    if let Some(launch) = launches
        .iter_mut()
        .find(|launch| launch.recipient == recipient)
    {
        launch.status = status;
    }
}

pub(super) fn queued_message_recipient(message: &Value) -> Option<String> {
    let text = value_text(message.get("content"));
    if !text.contains("had no active task") {
        return None;
    }
    text.split_once("Agent \"")
        .and_then(|(_, value)| value.split_once('"'))
        .map(|(recipient, _)| recipient.to_owned())
        .filter(|recipient| !recipient.is_empty())
}

pub(super) fn status_update(message: &Value) -> Option<(String, String)> {
    let text = value_text(message.get("content"));
    let task_id = xml_value(&text, "task-id")?;
    let status = xml_value(&text, "status")?;
    Some((task_id, status))
}

pub(super) fn launch_record(
    block: &Value,
    contexts: &HashMap<String, (String, Option<String>, Option<String>)>,
) -> Option<LaunchRecord> {
    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
        return None;
    }
    let text = value_text(block.get("content"));
    if !is_launch_result(&text) {
        return None;
    }
    let recipient = find_recipient(block).or_else(|| parse_recipient(&text))?;
    let key = block
        .get("tool_use_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| recipient.clone());
    let context = block
        .get("tool_use_id")
        .and_then(Value::as_str)
        .and_then(|id| contexts.get(id))
        .cloned()
        .unwrap_or_default();
    Some(LaunchRecord {
        key,
        recipient,
        scope: context.0,
        model: context.1,
        status: super::records_scope::active_status(),
    })
}
