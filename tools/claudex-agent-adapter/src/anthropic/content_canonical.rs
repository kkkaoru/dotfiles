use serde_json::{Map, Value};

pub(in crate::anthropic) fn canonical_eq(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| canonical_eq(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            let left_len = left
                .keys()
                .filter(|key| key.as_str() != "cache_control")
                .count();
            let right_len = right
                .keys()
                .filter(|key| key.as_str() != "cache_control")
                .count();
            left_len == right_len
                && left.iter().all(|(key, value)| {
                    key == "cache_control"
                        || right
                            .get(key)
                            .is_some_and(|right| canonical_eq(value, right))
                })
        }
        _ => left == right,
    }
}

pub(in crate::anthropic) fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        Value::Object(values) => canonical_object(values),
        value => value.clone(),
    }
}

pub(in crate::anthropic) fn canonical_object(values: &Map<String, Value>) -> Value {
    Value::Object(
        values
            .iter()
            .filter(|(key, _)| key.as_str() != "cache_control")
            .map(|(key, value)| (key.clone(), canonical_value(value)))
            .collect(),
    )
}

pub(in crate::anthropic) fn system_text(system: &Value) -> String {
    content_text(system)
}

pub(in crate::anthropic) fn content_text(content: &Value) -> String {
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

pub(in crate::anthropic) fn text_block(block: &Value) -> Option<&str> {
    (block.get("type").and_then(Value::as_str) == Some("text"))
        .then(|| block.get("text").and_then(Value::as_str))
        .flatten()
}

pub(in crate::anthropic) fn image_data_url(block: &Value) -> Option<String> {
    let source = block.get("source")?;
    match source.get("type")?.as_str()? {
        "base64" => Some(format!(
            "data:{};base64,{}",
            source.get("media_type")?.as_str()?,
            source.get("data")?.as_str()?
        )),
        "url" => source.get("url")?.as_str().map(str::to_owned),
        _ => None,
    }
}
