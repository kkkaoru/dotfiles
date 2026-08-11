use serde_json::Value;

use super::{COMPACTION_COMMAND_TAG, COMPACTION_SUMMARY_TASK, COMPACTION_TEXT_ONLY_PREFIX};

pub(super) fn message_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(text_block)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

pub(super) fn text_block(block: &Value) -> Option<&str> {
    (block.get("type").and_then(Value::as_str) == Some("text"))
        .then(|| block.get("text").and_then(Value::as_str))
        .flatten()
}

pub(super) fn is_compaction_text(text: &str) -> bool {
    let compact_command = text
        .strip_prefix("/compact")
        .is_some_and(|tail| tail.chars().next().is_none_or(char::is_whitespace));
    compact_command
        || text.starts_with(COMPACTION_COMMAND_TAG)
        || (text.starts_with(COMPACTION_TEXT_ONLY_PREFIX) && text.contains(COMPACTION_SUMMARY_TASK))
}
