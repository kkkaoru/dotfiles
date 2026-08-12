use serde_json::Value;

use crate::anthropic::MessagesRequest;

use super::{
    actions::declines_delegation_text,
    detect::is_remaining_only_follow_up,
    filters::{
        classifiable_text, is_generated_instruction, remove_fenced_and_blockquoted_text,
        remove_inline_quoted_text,
    },
};

pub(super) fn last_real_user_text(request: &MessagesRequest) -> Option<String> {
    let classifiable = request
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|(index, _)| user_message_text(&request.messages, index))
        .collect::<Vec<_>>();
    let latest = classifiable.last()?;
    if declines_delegation_text(latest) {
        return Some(latest.clone());
    }
    if is_remaining_only_follow_up(latest) {
        return classifiable
            .iter()
            .rev()
            .find(|content| !is_remaining_only_follow_up(content))
            .cloned()
            .or_else(|| Some(latest.clone()));
    }
    Some(latest.clone())
}

fn user_message_text(messages: &[Value], index: usize) -> Option<String> {
    let message = messages.get(index)?;
    if has_non_user_provenance(message) || is_generated_skill_payload(messages, index) {
        return None;
    }
    match message.get("content")? {
        Value::String(text) => classifiable_text(text),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter(|block| !has_non_user_provenance(block))
                .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .filter_map(classifiable_text)
                .collect::<Vec<_>>()
                .join("\n");
            (!text.trim().is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn is_generated_skill_payload(messages: &[Value], index: usize) -> bool {
    let Some(message) = messages.get(index) else {
        return false;
    };
    let raw_text = raw_message_text(message);
    if raw_text.trim().is_empty() {
        return false;
    }
    let visible_text = remove_inline_quoted_text(&remove_fenced_and_blockquoted_text(&raw_text));
    let has_command_marker = visible_text.lines().any(is_exact_generated_command_line);
    let has_skill_base = visible_text.lines().any(|line| {
        line.trim_start()
            .starts_with("Base directory for this skill:")
    });
    (has_command_marker && has_skill_base)
        || (follows_skill_invocation(messages, index) && looks_like_skill_document(&visible_text))
}

fn raw_message_text(message: &Value) -> String {
    match message.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn is_exact_generated_command_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("<command-message>")
        || trimmed.starts_with("<command-name>")
        || trimmed.starts_with("(Re-invocation of /")
        || trimmed.starts_with("Launching skill:")
}

fn follows_skill_invocation(messages: &[Value], index: usize) -> bool {
    for message in messages[..index].iter().rev() {
        match message.get("role").and_then(Value::as_str) {
            Some("assistant") => return assistant_invoked_skill(message),
            Some("user") if is_generated_chain_message(message) => continue,
            _ => return false,
        }
    }
    false
}

fn is_generated_chain_message(message: &Value) -> bool {
    if has_non_user_provenance(message) {
        return true;
    }
    match message.get("content") {
        Some(Value::String(text)) => is_generated_instruction(text),
        Some(Value::Array(blocks)) if !blocks.is_empty() => blocks.iter().all(|block| {
            has_non_user_provenance(block)
                || block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(is_generated_instruction)
        }),
        _ => false,
    }
}

fn assistant_invoked_skill(message: &Value) -> bool {
    message
        .get("content")
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && block
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case("skill"))
            })
        })
}

fn looks_like_skill_document(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.lines().any(|line| {
        line.trim_start()
            .starts_with("Base directory for this skill:")
    }) || (trimmed.starts_with("# /")
        && (trimmed.contains("\n## Input") || trimmed.contains("\n# Input")))
}

fn has_non_user_provenance(value: &Value) -> bool {
    ["isMeta", "is_meta"]
        .iter()
        .any(|field| value.get(field).and_then(Value::as_bool) == Some(true))
        || ["sourceToolUseID", "source_tool_use_id", "attributionSkill"]
            .iter()
            .any(|field| value.get(field).is_some_and(|marker| !marker.is_null()))
        || value
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(is_non_user_block_type)
        || value
            .get("attachment")
            .and_then(|attachment| attachment.get("type"))
            .and_then(Value::as_str)
            .is_some_and(is_non_user_block_type)
}

fn is_non_user_block_type(block_type: &str) -> bool {
    matches!(
        block_type,
        "tool_result"
            | "task_reminder"
            | "task_notification"
            | "system_reminder"
            | "system"
            | "attachment"
            | "hook_additional_context"
            | "lifecycle"
    )
}

#[cfg(test)]
include!("content_tests.rs");
