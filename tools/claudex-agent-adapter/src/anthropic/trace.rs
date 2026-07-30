use super::MessagesRequest;
use serde_json::Value;

pub(crate) fn trace_request(request: &MessagesRequest) -> bool {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return false;
    }
    let tool_names = request
        .tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    let tool_result_ids = request
        .messages
        .iter()
        .flat_map(|message| message["content"].as_array())
        .flatten()
        .filter(|block| block["type"] == "tool_result")
        .filter_map(|block| block["tool_use_id"].as_str())
        .collect::<Vec<_>>();
    let tool_result_shapes = request
        .messages
        .iter()
        .flat_map(|message| message["content"].as_array())
        .flatten()
        .filter(|block| block["type"] == "tool_result")
        .map(|block| {
            let content = match block.get("content") {
                Some(Value::String(_)) => "string".to_owned(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| {
                        let kind = item["type"].as_str().unwrap_or("object");
                        let keys = item
                            .as_object()
                            .map(|object| object.keys().cloned().collect::<Vec<_>>())
                            .unwrap_or_default();
                        format!("{kind}:{keys:?}")
                    })
                    .collect::<Vec<_>>()
                    .join(","),
                Some(Value::Object(_)) => "object".to_owned(),
                Some(_) => "value".to_owned(),
                None => "missing".to_owned(),
            };
            format!(
                "error={}:content={content:?}",
                block["is_error"].as_bool().unwrap_or(false)
            )
        })
        .collect::<Vec<_>>();
    let tool_result_string_shapes = request
        .messages
        .iter()
        .flat_map(|message| message["content"].as_array())
        .flatten()
        .filter(|block| block["type"] == "tool_result")
        .filter_map(|block| block["content"].as_str())
        .map(string_shape)
        .collect::<Vec<_>>();
    let message_shapes = request
        .messages
        .iter()
        .map(|message| {
            let role = message["role"].as_str().unwrap_or("?");
            let kinds = message["content"].as_array().map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|block| block["type"].as_str().or_else(|| block["name"].as_str()))
                    .collect::<Vec<_>>()
            });
            format!("{role}:{kinds:?}")
        })
        .collect::<Vec<_>>();
    tracing::debug!(
        request_model = %request.model,
        stream = request.stream,
        system_bytes = serialized_len(&request.system),
        message_bytes = serialized_len(&request.messages),
        tool_count = request.tools.len(),
        tool_bytes = serialized_len(&request.tools),
        ?tool_names,
        ?tool_result_ids,
        ?tool_result_shapes,
        ?tool_result_string_shapes,
        ?message_shapes,
        output_config = %request.output_config,
        "received Claude Code Messages request"
    );
    true
}

fn string_shape(text: &str) -> String {
    let first = text.chars().next().unwrap_or(' ');
    let parsed = serde_json::from_str::<Value>(text).ok();
    let json_shape = parsed.as_ref().map(value_shape);
    format!("len={},first={first:?},json={json_shape:?}", text.len())
}

fn value_shape(value: &Value) -> String {
    match value {
        Value::Array(items) => format!("array:{}", items.len()),
        Value::Object(object) => {
            let mut keys = object.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            format!("object:{keys:?}")
        }
        Value::String(_) => "string".to_owned(),
        Value::Bool(_) => "bool".to_owned(),
        Value::Number(_) => "number".to_owned(),
        Value::Null => "null".to_owned(),
    }
}

fn serialized_len(value: &impl serde::Serialize) -> usize {
    serde_json::to_vec(value).map_or(0, |bytes| bytes.len())
}
