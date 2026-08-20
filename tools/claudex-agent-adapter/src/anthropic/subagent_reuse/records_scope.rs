use std::collections::HashSet;

use serde_json::Value;

#[cfg(test)]
use super::value_text;

#[cfg(test)]
pub(super) fn latest_user_text(messages: &[Value]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .map(|message| value_text(message.get("content")))
        .unwrap_or_default()
}

#[cfg(test)]
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

const SCOPE_CHAR_LIMIT: usize = 180;
const SOURCE_EXTENSIONS: &[&str] = &[
    "c", "cc", "cpp", "css", "fish", "go", "h", "html", "java", "js", "json", "jsx", "kt", "md",
    "mjs", "py", "rb", "rs", "sh", "swift", "toml", "ts", "tsx",
];

pub(super) fn summarize_scope(input: &Value) -> String {
    // Claude Code's agents panel titles `description`. Two workers with the same
    // card title are the same scope even when prompts differ by provider.
    // Persist mentioned source paths so a later same-file writer can resume.
    combine_scope(scope_title(input), mentioned_source_paths_from_input(input))
}

fn scope_title(input: &Value) -> String {
    let text = input
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| input.get("prompt").and_then(Value::as_str))
        .unwrap_or_default();
    text.lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with("claudex_")
                && !trimmed.starts_with("<claudex-")
        })
        .take(2)
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
}

fn mentioned_source_paths_from_input(input: &Value) -> Vec<String> {
    let description = input
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let prompt = input.get("prompt").and_then(Value::as_str).unwrap_or("");
    mentioned_source_paths(&format!("{description} {prompt}"))
}

pub(super) fn title_key(scope: &str) -> String {
    scope
        .split(is_path_separator)
        .filter(|token| source_path_from_token(token).is_none())
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

pub(super) fn share_source_path(left: &str, right: &str) -> bool {
    let left_paths = mentioned_source_paths(left);
    let right_paths = mentioned_source_paths(right);
    !left_paths.is_empty()
        && !right_paths.is_empty()
        && left_paths
            .iter()
            .any(|left| path_is_mentioned(left, &right_paths))
}

fn path_is_mentioned(left: &str, right_paths: &[String]) -> bool {
    right_paths
        .iter()
        .any(|right| same_source_path(left, right))
}

fn mentioned_source_paths(text: &str) -> Vec<String> {
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for token in text.split(is_path_separator) {
        let Some(path) = source_path_from_token(token) else {
            continue;
        };
        if seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    paths
}

fn source_path_from_token(token: &str) -> Option<String> {
    let trimmed =
        token.trim_matches(|character: char| matches!(character, '.' | ':' | '!' | '?' | '*'));
    let trimmed = trimmed.trim_start_matches("./").replace('\\', "/");
    if trimmed.is_empty() || trimmed.contains("://") {
        return None;
    }
    let (stem, extension) = trimmed.rsplit_once('.')?;
    if stem.is_empty()
        || !is_source_extension(extension)
        || !stem
            .chars()
            .any(|character| character.is_ascii_alphabetic())
    {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

fn is_source_extension(extension: &str) -> bool {
    SOURCE_EXTENSIONS
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

fn is_path_separator(character: char) -> bool {
    character.is_whitespace()
        || matches!(
            character,
            ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | '`' | '<' | '>' | '|'
        )
}

fn same_source_path(left: &str, right: &str) -> bool {
    left == right || left.ends_with(&format!("/{right}")) || right.ends_with(&format!("/{left}"))
}

fn combine_scope(title: String, paths: Vec<String>) -> String {
    if paths.is_empty() {
        return title.chars().take(SCOPE_CHAR_LIMIT).collect();
    }
    let extra = extra_source_paths(&title, &paths);
    if extra.is_empty() {
        return title.chars().take(SCOPE_CHAR_LIMIT).collect();
    }
    let path_text = extra.join(" ");
    let budget = SCOPE_CHAR_LIMIT.saturating_sub(path_text.len().saturating_add(1));
    let truncated: String = title.chars().take(budget).collect();
    if truncated.trim().is_empty() {
        path_text.chars().take(SCOPE_CHAR_LIMIT).collect()
    } else {
        format!("{} {path_text}", truncated.trim())
    }
}

fn extra_source_paths(title: &str, paths: &[String]) -> Vec<String> {
    let lowered = title.to_ascii_lowercase();
    paths
        .iter()
        .filter(|path| !lowered.contains(path.as_str()))
        .cloned()
        .collect()
}

pub(super) fn find_recipient(value: &Value) -> Option<String> {
    match value {
        Value::Object(object) => object.iter().find_map(|(key, value)| {
            if matches!(key.as_str(), "agentId" | "agent_id" | "taskId" | "task_id") {
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
                .trim_matches(|character: char| matches!(character, '\'' | '"' | '`' | ',' | ')'));
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
