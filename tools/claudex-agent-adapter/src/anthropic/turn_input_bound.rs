use serde_json::{Value, json};

use super::{TRUNCATED_INPUT_NOTICE};
use crate::anthropic::content::{image_data_url, serialized_len};

pub(super) fn bound_input_with_byte_limit(input: Vec<Value>, max_bytes: usize) -> Vec<Value> {
    let original_bytes = input.iter().map(input_bytes).sum::<usize>();
    if original_bytes <= max_bytes {
        return input;
    }
    let mut remaining = max_bytes.saturating_sub(TRUNCATED_INPUT_NOTICE.len());
    let mut retained = Vec::new();
    for item in input.into_iter().rev() {
        let size = input_bytes(&item);
        if size <= remaining {
            remaining -= size;
            retained.push(item);
        } else if retained.is_empty() {
            retain_truncated_item(&mut retained, item, remaining);
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

pub(super) fn retain_truncated_item(retained: &mut Vec<Value>, mut item: Value, remaining: usize) {
    let Some(text) = item.get_mut("text") else {
        return;
    };
    let suffix = text
        .as_str()
        .map(|value| utf8_suffix(value, remaining).to_owned())
        .unwrap_or_default();
    *text = json!(suffix);
    retained.push(item);
}

pub(super) fn input_bytes(item: &Value) -> usize {
    ["text", "url"]
        .into_iter()
        .find_map(|field| item.get(field).and_then(Value::as_str))
        .map_or_else(|| serialized_len(item), str::len)
}

pub(super) fn message_input(message: &Value) -> Vec<Value> {
    match message.get("content") {
        Some(Value::String(text)) => vec![json!({"type":"text", "text":text})],
        Some(Value::Array(blocks)) => blocks.iter().filter_map(input_block).collect(),
        _ => Vec::new(),
    }
}

pub(super) fn input_block(block: &Value) -> Option<Value> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => Some(json!({
            "type":"text", "text":block.get("text").and_then(Value::as_str).unwrap_or("")
        })),
        Some("image") => image_data_url(block).map(|url| json!({"type":"image", "url":url})),
        _ => None,
    }
}

pub(super) fn utf8_suffix(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len().saturating_sub(max_bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}
