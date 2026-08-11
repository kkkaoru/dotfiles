use serde_json::{Value, json};

use super::{ToolResult, image_data_url, input_text};

pub(super) fn tool_result(block: &Value) -> Option<ToolResult> {
    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
        return None;
    }
    let tool_use_id = block.get("tool_use_id")?.as_str()?.to_owned();
    let mut content_items = tool_result_content(block.get("content"));
    if content_items.is_empty() {
        content_items.push(json!({ "type": "inputText", "text": "" }));
    }
    Some(ToolResult {
        tool_use_id,
        content_items,
        is_error: block
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn tool_result_content(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => vec![input_text(text)],
        Some(Value::Array(items)) => items.iter().filter_map(tool_result_item).collect(),
        _ => Vec::new(),
    }
}

fn tool_result_item(item: &Value) -> Option<Value> {
    match item.get("type").and_then(Value::as_str) {
        Some("text") => Some(input_text(
            item.get("text").and_then(Value::as_str).unwrap_or(""),
        )),
        Some("image") => image_data_url(item)
            .map(|image_url| json!({ "type": "inputImage", "imageUrl": image_url })),
        _ => None,
    }
}
