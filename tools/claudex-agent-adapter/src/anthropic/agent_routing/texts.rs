use serde::Deserialize;
use serde_json::Value;

pub(super) fn routing_summary(text: &str) -> Option<Value> {
    let start = text.find("{\"providers\":")?;
    Value::deserialize(&mut serde_json::Deserializer::from_str(&text[start..])).ok()
}

pub(super) fn user_message_texts(messages: &[Value]) -> impl Iterator<Item = &str> {
    messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content"))
        .flat_map(value_texts)
}

pub(super) fn message_texts(messages: &[Value]) -> impl Iterator<Item = &str> {
    messages
        .iter()
        .filter_map(|message| message.get("content"))
        .flat_map(value_texts)
}

pub(super) fn value_texts(value: &Value) -> impl Iterator<Item = &str> {
    value.as_str().into_iter().chain(
        value
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| block.get("text").and_then(Value::as_str)),
    )
}
