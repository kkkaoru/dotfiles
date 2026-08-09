use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::{is_launch_tool, value_text};

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
            current.key == launch.key
                || current.recipient == launch.recipient
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
    // selected_workers is a capacity pool, not a launch count. Same scope stays
    // one in-flight worker even when the orchestrator picks another model.
    normalize_scope(&current.scope) == normalize_scope(&observed.scope)
}

pub(super) fn launch_scope_key(input: &Value) -> String {
    normalize_scope(&summarize_scope(input))
}

pub(super) fn scope_is_occupied(launches: &[LaunchRecord], scope_key: &str) -> bool {
    !scope_key.is_empty()
        && launches.iter().any(|launch| {
            !terminal_status(&launch.status) && normalize_scope(&launch.scope) == scope_key
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
        .filter(|current| {
            reusable_status(&current.status)
                && !current.recipient.is_empty()
                && same_logical_launch(current, &proposed)
        })
        .collect::<Vec<_>>();
    if exact.is_empty() {
        return None;
    }
    exact.sort_by_key(|launch| std::cmp::Reverse(reuse_priority(&launch.status)));
    exact.into_iter().next()
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
    if current.scope.is_empty() {
        current.scope.clone_from(&observed.scope);
    }
    if current.model.is_none() {
        current.model.clone_from(&observed.model);
    }
    if !observed.status.is_empty() {
        current.status.clone_from(&observed.status);
    }
}

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

pub(super) fn apply_transcript(launches: &mut Vec<LaunchRecord>, messages: &[Value]) {
    let mut contexts = HashMap::new();
    for message in messages {
        if let Some(content) = message.get("content") {
            let blocks = content.as_array().into_iter().flatten();
            for block in blocks {
                remember_launch_context(&mut contexts, block);
                if let Some(record) = launch_record(block, &contexts) {
                    merge_launches(launches, std::iter::once(&record));
                }
            }
        }
        if let Some((task_id, status)) = status_update(message) {
            set_task_status(launches, &task_id, status);
            continue;
        }
        if let Some(recipient) = queued_message_recipient(message) {
            set_recipient_status(launches, &recipient, "message_queued".to_owned());
        }
    }
}

fn remember_launch_context(
    contexts: &mut HashMap<String, (String, Option<String>)>,
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
    contexts.insert(
        id.to_owned(),
        (
            summarize_scope(input),
            input
                .get("claudex_model")
                .and_then(Value::as_str)
                .map(str::to_owned),
        ),
    );
}

fn launch_record(
    block: &Value,
    contexts: &HashMap<String, (String, Option<String>)>,
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

pub(super) fn latest_user_text(messages: &[Value]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .map(|message| value_text(message.get("content")))
        .unwrap_or_default()
}

pub(super) fn scope_similarity(scope: &str, task: &str) -> usize {
    let task_words = task
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.len() >= 3)
        .map(str::to_ascii_lowercase)
        .collect::<HashSet<_>>();
    scope
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| word.len() >= 3)
        .map(str::to_ascii_lowercase)
        .filter(|word| task_words.contains(word))
        .count()
}

pub(super) fn summarize_scope(input: &Value) -> String {
    // Claude Code's agents panel titles `description`. Two workers with the same
    // card title are the same scope even when prompts differ by provider.
    let text = input
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| input.get("prompt").and_then(Value::as_str))
        .unwrap_or_default();
    let summary = text
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with("claudex_")
                && !trimmed.starts_with("<claudex-")
        })
        .take(2)
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ");
    summary.chars().take(180).collect()
}

fn find_recipient(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => object.iter().find_map(|(key, value)| {
            if matches!(key.as_str(), "agentId" | "agent_id") {
                return value
                    .as_str()
                    .filter(|recipient| !recipient.is_empty())
                    .map(str::to_owned);
            }
            find_recipient(value)
        }),
        Value::Array(values) => values.iter().find_map(find_recipient),
        _ => None,
    }
}

fn parse_recipient(text: &str) -> Option<String> {
    ["agentId:", "agent_id:"].into_iter().find_map(|marker| {
        let value = text.split_once(marker)?.1.lines().next()?.trim();
        let value = value
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|character: char| matches!(character, '\'' | '"' | '`' | ',' | ')'));
        (!value.is_empty()).then(|| value.to_owned())
    })
}

fn is_launch_result(text: &str) -> bool {
    text.contains("Async agent launched")
        || text.contains("teammate_spawned")
        || text.contains("working in the background")
        || text.contains("receives instructions via mailbox")
}

fn xml_value(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let value = text.split_once(&open)?.1.split_once(&close)?.0.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn active_status() -> String {
    "active".to_owned()
}
