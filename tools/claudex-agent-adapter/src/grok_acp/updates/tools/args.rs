use agent_client_protocol as acp;
use serde_json::{Map, Value, json};

use super::super::tools_labels::tool_content_text;

pub(super) fn build_tool_input(call: &acp::ToolCall) -> Value {
    enrich_arguments(
        call.raw_input
            .clone()
            .unwrap_or_else(|| json!({"description": call.title})),
        &Some(call.content.clone()),
        &Some(call.locations.clone()),
    )
}

pub(super) fn enrich_arguments(
    raw_input: Value,
    content: &Option<Vec<acp::ToolCallContent>>,
    locations: &Option<Vec<acp::ToolCallLocation>>,
) -> Value {
    let mut object = match raw_input {
        Value::Object(map) => map,
        other if !other.is_null() => {
            let mut map = Map::new();
            map.insert("value".into(), other);
            map
        }
        _ => Map::new(),
    };
    if let Some(paths) = locations.as_ref().filter(|items| !items.is_empty()) {
        object.insert(
            "locations".into(),
            Value::Array(paths.iter().map(tool_location).collect()),
        );
    }
    if let Some(content) = content {
        let text = tool_content_text(content);
        if !text.is_empty() {
            object.entry("content".to_owned()).or_insert(json!(text));
        }
    }
    if object.is_empty() {
        json!({})
    } else {
        Value::Object(object)
    }
}

pub(super) fn tool_location(location: &acp::ToolCallLocation) -> Value {
    let mut entry = json!({"path": location.path.display().to_string()});
    if let Some(line) = location.line {
        entry["line"] = json!(line);
    }
    entry
}

pub(super) fn combine_output(
    raw_output: Option<Value>,
    content: Option<&Vec<acp::ToolCallContent>>,
) -> Option<Value> {
    let content_text = content
        .map(|items| tool_content_text(items.as_slice()))
        .unwrap_or_default();
    match (raw_output, content_text.as_str()) {
        (Some(Value::String(s)), extra) if !extra.is_empty() && s != extra => {
            Some(json!(format!("{s}\n{extra}")))
        }
        (Some(value), _) => Some(value),
        (None, extra) if !extra.is_empty() => Some(json!(extra)),
        _ => None,
    }
}
