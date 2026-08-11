use serde_json::{Value, json};

use super::super::content::serialized_len;
use super::{FULL_HISTORY_HEADER, MAX_TURN_INPUT_BYTES, TRUNCATED_HISTORY_HEADER, utf8_suffix};

pub(in crate::anthropic) fn bounded_history(messages: &[Value], max_bytes: usize) -> (&'static str, String) {
    let original_bytes = serialized_len(&messages);
    if original_bytes + FULL_HISTORY_HEADER.len() <= max_bytes {
        return (
            FULL_HISTORY_HEADER,
            serde_json::to_string(messages).unwrap_or_default(),
        );
    }
    let budget = max_bytes.saturating_sub(TRUNCATED_HISTORY_HEADER.len());
    let mut start = messages.len();
    let mut retained_bytes = 2;
    for (index, message) in messages.iter().enumerate().rev() {
        let separator = usize::from(start < messages.len());
        let next = serialized_len(message) + separator;
        if retained_bytes + next > budget {
            break;
        }
        retained_bytes += next;
        start = index;
    }
    let history = if start == messages.len() {
        oversized_latest_message(messages.last(), budget)
    } else {
        serde_json::to_string(&messages[start..]).unwrap_or_default()
    };
    tracing::warn!(
        original_bytes,
        retained_messages = messages.len().saturating_sub(start),
        "truncated reconstructed transcript before provider turn/start"
    );
    (TRUNCATED_HISTORY_HEADER, history)
}

pub(in crate::anthropic) fn oversized_latest_message(message: Option<&Value>, budget: usize) -> String {
    let Some(message) = message else {
        return "[]".to_owned();
    };
    let serialized = serde_json::to_string(message).unwrap_or_default();
    let excerpt = utf8_suffix(&serialized, budget.min(MAX_TURN_INPUT_BYTES / 4));
    serde_json::to_string(&json!([{
        "role":message.get("role").and_then(Value::as_str).unwrap_or("unknown"),
        "truncated_message_suffix":excerpt
    }]))
    .unwrap_or_else(|_| "[]".to_owned())
}

