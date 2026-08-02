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
        match launches
            .iter_mut()
            .find(|current| current.key == launch.key)
        {
            Some(current) => merge_record(current, launch),
            None => launches.push(launch.clone()),
        }
    }
}

fn merge_record(current: &mut LaunchRecord, observed: &LaunchRecord) {
    if current.scope.is_empty() {
        current.scope.clone_from(&observed.scope);
    }
    if current.model.is_none() {
        current.model.clone_from(&observed.model);
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

pub(super) fn update_status_from_notifications(launches: &mut [LaunchRecord], messages: &[Value]) {
    for message in messages {
        let Some((task_id, status)) = status_update(message) else {
            continue;
        };
        if let Some(launch) = launches
            .iter_mut()
            .find(|launch| launch.key == task_id || launch.recipient == task_id)
        {
            launch.status = status;
        }
    }
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

fn summarize_scope(input: &Value) -> String {
    let text = input
        .get("prompt")
        .or_else(|| input.get("description"))
        .and_then(Value::as_str)
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
