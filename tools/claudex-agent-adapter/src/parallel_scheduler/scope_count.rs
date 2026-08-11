use serde_json::Value;

use crate::anthropic::MessagesRequest;

#[path = "scope_count_detect.rs"]
mod detect;
use detect::{
    contains_parallel_intent, contains_single_scope_request, contains_substantive_verb,
    count_explicit_blocks, explicit_scope_cardinality, is_remaining_only_follow_up,
};

pub(super) const MAX_STATED_SCOPES: usize = 40;
pub(super) const CARDINALITY_WINDOW: usize = 24;

pub(crate) fn has_parallel_scope(request: &MessagesRequest) -> bool {
    independent_scope_count(request) >= 2
}

pub(crate) fn independent_scope_count(request: &MessagesRequest) -> usize {
    match last_real_user_text(request) {
        Some(content) => count_for_content(&content),
        // Reconstructed transcripts keep a parallel baseline for floor/replenishment.
        None => 2,
    }
}

pub(super) fn has_classifiable_user_turn(request: &MessagesRequest) -> bool {
    last_real_user_text(request).is_some()
}

fn last_real_user_text(request: &MessagesRequest) -> Option<String> {
    request
        .messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(user_message_text)
        .filter(|content| !content.trim_start().starts_with("<task-notification>"))
        .rev()
        .find(|content| !is_remaining_only_follow_up(content))
}

fn user_message_text(message: &Value) -> Option<String> {
    match message.get("content")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(|block| {
                    (block.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| block.get("text").and_then(Value::as_str))
                        .flatten()
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn count_for_content(content: &str) -> usize {
    if contains_single_scope_request(content) {
        return 1;
    }
    if let Some(stated) = explicit_scope_cardinality(content) {
        return stated;
    }
    let explicit_blocks = count_explicit_blocks(content);
    if explicit_blocks >= 2 {
        return explicit_blocks;
    }
    if contains_parallel_intent(content) {
        2
    } else {
        1
    }
}

pub(crate) fn needs_single_worker(request: &MessagesRequest) -> bool {
    if independent_scope_count(request) >= 2 {
        return false;
    }
    request
        .messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(user_message_text)
        .any(|content| is_atomic_lookup(&content))
}

fn is_atomic_lookup(content: &str) -> bool {
    if contains_single_scope_request(content) {
        return true;
    }
    let trimmed = content.trim();
    let lower = trimmed.to_ascii_lowercase();
    let first = lower.split_whitespace().next().unwrap_or("");
    matches!(first, "gh" | "git" | "curl" | "wget")
        || ((lower.starts_with("http://") || lower.starts_with("https://"))
            && !trimmed.contains('\n')
            && trimmed.split_whitespace().count() <= 3)
}

pub(super) fn is_substantive_work(request: &MessagesRequest) -> bool {
    let Some(content) = last_real_user_text(request) else {
        return false;
    };
    count_for_content(&content) >= 2
        || contains_parallel_intent(&content)
        || contains_substantive_verb(&content)
}
