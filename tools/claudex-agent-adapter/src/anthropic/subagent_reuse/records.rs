use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::{
    is_launch_tool,
    records_scope::{active_status, find_recipient, is_launch_result, parse_recipient, xml_value},
    value_text,
};

pub(super) use super::records_scope::summarize_scope;

#[derive(Clone, Debug, Default, serde::Deserialize, Eq, PartialEq, serde::Serialize)]
pub(super) struct LaunchRecord {
    pub(super) key: String,
    pub(super) recipient: String,
    #[serde(default)]
    pub(super) scope: String,
    #[serde(default)]
    pub(super) model: Option<String>,
    #[serde(default = "active_status")]
    pub(super) status: String,
}

pub(super) fn merge_launches<'a>(
    launches: &mut Vec<LaunchRecord>,
    observed: impl Iterator<Item = &'a LaunchRecord>,
) {
    for launch in observed {
        match launches.iter_mut().find(|current| {
            // Empty key/recipient are placeholders (inflight note before
            // tool_result). Matching ""=="" collapsed parallel SubAgents into
            // one record and broke occupancy / resume / spawn limits.
            (!current.key.is_empty() && current.key == launch.key)
                || (!current.recipient.is_empty()
                    && !launch.recipient.is_empty()
                    && current.recipient == launch.recipient)
                || (same_logical_launch(current, launch) && !terminal_status(&current.status))
        }) {
            Some(current) => merge_record(current, launch),
            None => launches.push(launch.clone()),
        }
    }
}

fn same_logical_launch(current: &LaunchRecord, observed: &LaunchRecord) -> bool {
    if current.scope.is_empty() || observed.scope.is_empty() {
        return false;
    }
    if normalize_scope(&current.scope) != normalize_scope(&observed.scope) {
        return false;
    }
    // Same agents-panel title with different claudex_model is intentional
    // multi-route fan-out (gpt + cursor + muse + advisor), not a duplicate.
    match (&current.model, &observed.model) {
        (Some(left), Some(right)) => left.eq_ignore_ascii_case(right),
        (None, None) => true,
        _ => false,
    }
}

pub(super) fn launch_scope_key(input: &Value) -> String {
    normalize_scope(&super::records_scope::summarize_scope(input))
}

pub(super) fn launch_model(input: &Value) -> Option<&str> {
    input
        .get("claudex_model")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

pub(super) fn scope_is_occupied(
    launches: &[LaunchRecord],
    scope_key: &str,
    model: Option<&str>,
) -> bool {
    !scope_key.is_empty()
        && launches.iter().any(|launch| {
            if terminal_status(&launch.status) || normalize_scope(&launch.scope) != scope_key {
                return false;
            }
            match (model, launch.model.as_deref()) {
                (Some(want), Some(have)) => want.eq_ignore_ascii_case(have),
                (None, None) => true,
                // Model-less placeholders must not block explicit multi-model
                // fan-out; model-less queries still collide with any occupant.
                (None, Some(_)) => true,
                (Some(_), None) => false,
            }
        })
}

fn normalize_scope(scope: &str) -> String {
    scope
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

pub(super) fn terminal_status(status: &str) -> bool {
    matches!(
        status,
        "completed" | "failed" | "cancelled" | "canceled" | "timeout" | "stopped"
    )
}

pub(super) fn reusable_status(status: &str) -> bool {
    matches!(status, "active" | "message_queued" | "completed")
}

pub(super) fn already_has_resume(arguments: &Value) -> bool {
    arguments
        .get("resume")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
}

pub(super) fn find_reusable_launch<'a>(
    launches: &'a [LaunchRecord],
    arguments: &Value,
) -> Option<&'a LaunchRecord> {
    let scope = summarize_scope(arguments);
    if scope.is_empty() {
        return None;
    }
    let model = arguments
        .get("claudex_model")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let proposed = LaunchRecord {
        key: String::new(),
        recipient: String::new(),
        scope,
        model,
        status: active_status(),
    };
    let mut exact = launches
        .iter()
        .enumerate()
        .filter(|(_, current)| {
            reusable_status(&current.status)
                && !current.recipient.is_empty()
                && same_logical_launch(current, &proposed)
        })
        .collect::<Vec<_>>();
    if exact.is_empty() {
        return None;
    }
    // Prefer higher status priority, then the newest record (prompt-cache /
    // latest transcript) when priorities tie.
    exact.sort_by(|(left_idx, left), (right_idx, right)| {
        reuse_priority(&right.status)
            .cmp(&reuse_priority(&left.status))
            .then_with(|| right_idx.cmp(left_idx))
    });
    exact.into_iter().next().map(|(_, launch)| launch)
}

fn reuse_priority(status: &str) -> u8 {
    match status {
        "active" => 2,
        "message_queued" => 1,
        "completed" => 0,
        _ => 0,
    }
}

fn merge_record(current: &mut LaunchRecord, observed: &LaunchRecord) {
    if current.recipient.is_empty() && !observed.recipient.is_empty() {
        current.recipient.clone_from(&observed.recipient);
    }
    // Explicit resume / follow-up launches refresh scope so the next same-scope
    // rewrite still hits this recipient instead of spawning a peer.
    if !observed.scope.is_empty() {
        current.scope.clone_from(&observed.scope);
    }
    if current.model.is_none() {
        current.model.clone_from(&observed.model);
    }
    if !observed.status.is_empty() {
        current.status.clone_from(&observed.status);
    }
}
#[cfg(test)]
pub(super) fn launch_records(messages: &[Value]) -> Vec<LaunchRecord> {
    let mut records = Vec::new();
    let mut contexts = HashMap::new();
    for message in messages {
        let Some(content) = message.get("content") else {
            continue;
        };
        let blocks = content.as_array().into_iter().flatten();
        for block in blocks {
            remember_launch_context(&mut contexts, block);
            records.extend(launch_record(block, &contexts));
        }
    }
    records
}

pub(in crate::anthropic) fn live_agent_task_ids(messages: &[Value]) -> Vec<String> {
    let mut launches = Vec::new();
    apply_transcript(&mut launches, messages);
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    for launch in launches {
        if terminal_status(&launch.status) {
            continue;
        }
        push_live_candidates(&mut ids, &mut seen, &launch);
    }
    ids
}

fn push_live_candidates(ids: &mut Vec<String>, seen: &mut HashSet<String>, launch: &LaunchRecord) {
    for candidate in [&launch.recipient, &launch.key] {
        if !crate::anthropic::task_ids::is_claude_code_agent_task_id(candidate) {
            continue;
        }
        if seen.insert(candidate.to_ascii_lowercase()) {
            ids.push(candidate.clone());
        }
    }
}

pub(super) fn apply_transcript(launches: &mut Vec<LaunchRecord>, messages: &[Value]) {
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

fn apply_message_content(
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

fn remember_launch_context(
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
            super::records_scope::summarize_scope(input),
            input
                .get("claudex_model")
                .and_then(Value::as_str)
                .map(str::to_owned),
            resume,
        ),
    );
}

fn launch_record(
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
        status: active_status(),
    })
}
fn mark_failed_launch_result(
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

fn set_task_status(launches: &mut [LaunchRecord], task_id: &str, status: String) {
    if let Some(launch) = launches
        .iter_mut()
        .find(|launch| launch.key == task_id || launch.recipient == task_id)
    {
        launch.status = status;
    }
}

fn set_recipient_status(launches: &mut [LaunchRecord], recipient: &str, status: String) {
    if let Some(launch) = launches
        .iter_mut()
        .find(|launch| launch.recipient == recipient)
    {
        launch.status = status;
    }
}

fn queued_message_recipient(message: &Value) -> Option<String> {
    let text = value_text(message.get("content"));
    if !text.contains("had no active task") {
        return None;
    }
    text.split_once("Agent \"")
        .and_then(|(_, value)| value.split_once('"'))
        .map(|(recipient, _)| recipient.to_owned())
        .filter(|recipient| !recipient.is_empty())
}

fn status_update(message: &Value) -> Option<(String, String)> {
    let text = value_text(message.get("content"));
    let task_id = xml_value(&text, "task-id")?;
    let status = xml_value(&text, "status")?;
    Some((task_id, status))
}
