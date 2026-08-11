use std::collections::HashSet;

use serde_json::Value;

use super::value_text;

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

pub(super) fn find_recipient(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => object.iter().find_map(|(key, value)| {
            if matches!(
                key.as_str(),
                "agentId" | "agent_id" | "taskId" | "task_id"
            ) {
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

pub(super) fn parse_recipient(text: &str) -> Option<String> {
    ["agentId:", "agent_id:", "taskId:", "task_id:"]
        .into_iter()
        .find_map(|marker| {
            let value = text.split_once(marker)?.1.lines().next()?.trim();
            let value = value
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_matches(|character: char| {
                    matches!(character, '\'' | '"' | '`' | ',' | ')')
                });
            (!value.is_empty()).then(|| value.to_owned())
        })
}

pub(super) fn is_launch_result(text: &str) -> bool {
    text.contains("Async agent launched")
        || text.contains("teammate_spawned")
        || text.contains("working in the background")
        || text.contains("receives instructions via mailbox")
}

pub(super) fn xml_value(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let value = text.split_once(&open)?.1.split_once(&close)?.0.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

pub(super) fn active_status() -> String {
    "active".to_owned()
}
