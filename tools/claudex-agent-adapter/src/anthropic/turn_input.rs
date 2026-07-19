use serde_json::{Value, json};

use super::content::{image_data_url, serialized_len};

// Codex app-server rejects turn input above 1 MiB. Keep headroom for framing and count UTF-8
// bytes, which is conservative relative to the upstream character limit.
const MAX_TURN_INPUT_BYTES: usize = 3 * 256 * 1_024;
const FULL_HISTORY_HEADER: &str =
    "Continue this Claude Code conversation. The role-tagged history follows:\n";
const TRUNCATED_HISTORY_HEADER: &str = "Continue this Claude Code conversation. Earlier completed history was omitted to fit the provider input limit. The retained role-tagged history follows:\n";
const TRUNCATED_INPUT_NOTICE: &str =
    "[Earlier turn input was omitted to fit the provider input limit.]";

pub(super) fn full_transcript_input(messages: &[Value]) -> Vec<Value> {
    if messages.len() == 1 && messages[0].get("role").and_then(Value::as_str) == Some("user") {
        return user_input_from_messages(messages);
    }
    let (header, history) = bounded_history(messages);
    vec![json!({"type":"text", "text":format!("{header}{history}")})]
}

pub(super) fn user_input_from_messages(messages: &[Value]) -> Vec<Value> {
    let mut input = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .flat_map(message_input)
        .collect::<Vec<_>>();
    if input.is_empty() {
        input.push(json!({"type":"text", "text":"Continue."}));
    }
    bound_input(input)
}

fn bounded_history(messages: &[Value]) -> (&'static str, String) {
    let original_bytes = serialized_len(&messages);
    if original_bytes + FULL_HISTORY_HEADER.len() <= MAX_TURN_INPUT_BYTES {
        return (
            FULL_HISTORY_HEADER,
            serde_json::to_string(messages).unwrap_or_default(),
        );
    }
    let budget = MAX_TURN_INPUT_BYTES.saturating_sub(TRUNCATED_HISTORY_HEADER.len());
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
        "truncated reconstructed transcript before Codex turn/start"
    );
    (TRUNCATED_HISTORY_HEADER, history)
}

fn oversized_latest_message(message: Option<&Value>, budget: usize) -> String {
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

fn bound_input(input: Vec<Value>) -> Vec<Value> {
    let original_bytes = input.iter().map(input_bytes).sum::<usize>();
    if original_bytes <= MAX_TURN_INPUT_BYTES {
        return input;
    }
    let mut remaining = MAX_TURN_INPUT_BYTES.saturating_sub(TRUNCATED_INPUT_NOTICE.len());
    let mut retained = Vec::new();
    for mut item in input.into_iter().rev() {
        let size = input_bytes(&item);
        if size <= remaining {
            remaining -= size;
            retained.push(item);
        } else if retained.is_empty() {
            if let Some(text) = item.get_mut("text") {
                let suffix = text
                    .as_str()
                    .map(|value| utf8_suffix(value, remaining).to_owned())
                    .unwrap_or_default();
                *text = json!(suffix);
                retained.push(item);
            }
            break;
        } else {
            // Retain one contiguous suffix instead of stitching stale input around an
            // omitted oversized item.
            break;
        }
        if remaining == 0 {
            break;
        }
    }
    retained.reverse();
    retained.insert(0, json!({"type":"text", "text":TRUNCATED_INPUT_NOTICE}));
    tracing::warn!(
        original_bytes,
        retained_items = retained.len().saturating_sub(1),
        "truncated incremental input before Codex turn/start"
    );
    retained
}

fn input_bytes(item: &Value) -> usize {
    ["text", "url"]
        .into_iter()
        .find_map(|field| item.get(field).and_then(Value::as_str))
        .map_or_else(|| serialized_len(item), str::len)
}

fn message_input(message: &Value) -> Vec<Value> {
    match message.get("content") {
        Some(Value::String(text)) => vec![json!({"type":"text", "text":text})],
        Some(Value::Array(blocks)) => blocks.iter().filter_map(input_block).collect(),
        _ => Vec::new(),
    }
}

fn input_block(block: &Value) -> Option<Value> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => Some(json!({
            "type":"text", "text":block.get("text").and_then(Value::as_str).unwrap_or("")
        })),
        Some("image") => image_data_url(block).map(|url| json!({"type":"image", "url":url})),
        _ => None,
    }
}

fn utf8_suffix(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len().saturating_sub(max_bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

#[cfg(test)]
#[path = "turn_input_tests.rs"]
mod tests;
