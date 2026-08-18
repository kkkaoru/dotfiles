//! Fast-path handling for Claude Code's internal agent notifications.
//!
//! Native background Agent completion messages can arrive as a user-shaped
//! message. They are lifecycle signals, not new user instructions, so feeding
//! them through provider routing would make the next interactive prompt wait
//! behind an unnecessary turn.

use serde_json::{Value, json};

use super::MessagesRequest;

mod ack;
mod detect;
use ack::DEFAULT_NOTIFICATION_TEXT;
pub(super) use ack::{acknowledge, acknowledge_with_text};
use detect::{
    is_empty_user_content, is_internal_notification_content, is_internal_text_block,
    is_monitor_hint_text,
};

/// Remove pure Claude Code lifecycle notifications from the transcript before
/// it is reconstructed for a provider turn. Claude Code delivers these as
/// user-shaped messages, but they are not user instructions. Keeping them in
/// the transcript both inflates context and makes a delayed notification look
/// like a new user turn to a routed provider.
pub(super) fn remove_from_transcript(request: &mut MessagesRequest) {
    let messages = std::mem::take(&mut request.messages);
    let filtered = messages.into_iter().filter_map(retain_transcript_message);
    request.messages = separate_adjacent_assistants(filtered.collect());
}

fn retain_transcript_message(message: Value) -> Option<Value> {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return Some(message);
    }
    let Some(content) = message.get("content").cloned() else {
        return Some(message);
    };
    if is_internal_notification_content(&content) && !contains_tool_result(&content) {
        return None;
    }
    retain_non_notification_blocks(message, content)
}

fn contains_tool_result(content: &Value) -> bool {
    content
        .as_array()
        .is_some_and(|blocks| blocks.iter().any(is_tool_result_block))
}

fn is_tool_result_block(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("tool_result")
}

fn retain_non_notification_blocks(mut message: Value, content: Value) -> Option<Value> {
    let Value::Array(blocks) = content else {
        return Some(message);
    };
    let block_count = blocks.len();
    let filtered = blocks
        .into_iter()
        .filter(|block| !is_internal_text_block(block))
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        None
    } else if filtered.len() != block_count {
        message["content"] = Value::Array(filtered);
        Some(message)
    } else {
        Some(message)
    }
}

fn separate_adjacent_assistants(messages: Vec<Value>) -> Vec<Value> {
    let mut separated = Vec::with_capacity(messages.len());
    let mut previous_was_assistant = false;
    for message in messages {
        let is_assistant = message.get("role").and_then(Value::as_str) == Some("assistant");
        if previous_was_assistant && is_assistant {
            separated.push(json!({
                "role": "user",
                "content": DEFAULT_NOTIFICATION_TEXT,
            }));
        }
        previous_was_assistant = is_assistant;
        separated.push(message);
    }
    separated
}

pub(super) fn is_internal_notification_request(request: &MessagesRequest) -> bool {
    request
        .messages
        // Claude Code can append assistant/tool transcript elements after the
        // lifecycle user message when resuming a session. The last array
        // element is therefore not guaranteed to be the last user turn.
        .iter()
        .rev()
        // Empty user elements are transport/history separators, not a newer
        // instruction. Claude Code emits one of these around a queued
        // task-notification when a resumed session is refreshed.
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content"))
        .find(|content| !is_empty_user_content(content))
        .is_some_and(is_internal_notification_content)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "internal_notification_tests.rs"]
mod tests;
