use std::collections::HashMap;

use serde_json::Value;

use super::super::records_status::{
    mark_failed_launch_result, queued_message_recipient, set_recipient_status, set_task_status,
    status_update,
};
use super::{LaunchRecord, is_launch_tool, launch_record, merge_launches};

pub(in crate::anthropic::subagent_reuse) fn apply_transcript(
    launches: &mut Vec<LaunchRecord>,
    messages: &[Value],
) {
    let mut contexts = HashMap::new();
    for message in messages {
        apply_message_content(launches, &mut contexts, message);
        if let Some((task_id, status)) = status_update(message) {
            set_task_status(launches, &task_id, status);
            continue;
        }
        if let Some(recipient) = queued_message_recipient(message) {
            set_recipient_status(launches, &recipient, "message_queued".to_owned());
        }
    }
}

pub(super) fn apply_message_content(
    launches: &mut Vec<LaunchRecord>,
    contexts: &mut HashMap<String, (String, Option<String>, Option<String>)>,
    message: &Value,
) {
    let Some(content) = message.get("content") else {
        return;
    };
    for block in content.as_array().into_iter().flatten() {
        remember_launch_context(contexts, block);
        match launch_record(block, contexts) {
            Some(record) => merge_launches(launches, std::iter::once(&record)),
            None => mark_failed_launch_result(launches, contexts, block),
        }
    }
}

pub(in crate::anthropic::subagent_reuse) fn remember_launch_context(
    contexts: &mut HashMap<String, (String, Option<String>, Option<String>)>,
    block: &Value,
) {
    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
        return;
    }
    let Some(name) = block.get("name").and_then(Value::as_str) else {
        return;
    };
    if !is_launch_tool(name) {
        return;
    }
    let Some(id) = block.get("id").and_then(Value::as_str) else {
        return;
    };
    let input = block.get("input").unwrap_or(&Value::Null);
    let resume = input
        .get("resume")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    contexts.insert(
        id.to_owned(),
        (
            super::super::records_scope::summarize_scope(input),
            input
                .get("claudex_model")
                .and_then(Value::as_str)
                .map(str::to_owned),
            resume,
        ),
    );
}
