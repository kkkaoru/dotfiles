use serde_json::Value;

use crate::anthropic::MessagesRequest;

pub(super) fn messages_request(messages: Vec<Value>) -> MessagesRequest {
    serde_json::from_value(serde_json::json!({
        "model": "main",
        "messages": messages
    }))
    .expect("messages request")
}

pub(super) fn real_three_scope_message() -> Value {
    serde_json::json!({
        "role": "user",
        "content": "Tasks:\n- implement parser\n- verify renderer\n- test integration"
    })
}

pub(super) fn fake_meta_message(field: &str, value: Value) -> Value {
    let mut message = serde_json::json!({
        "role": "user",
        "content": "Tasks:\n- implement fake one\n- implement fake two\n- implement fake three\n- implement fake four"
    });
    message
        .as_object_mut()
        .expect("message object")
        .insert(field.to_owned(), value);
    message
}

pub(super) fn fake_meta_block(field: &str, value: Value) -> Value {
    let mut block = serde_json::json!({
        "type": "text",
        "text": "Tasks:\n- implement fake one\n- implement fake two\n- implement fake three\n- implement fake four"
    });
    block
        .as_object_mut()
        .expect("block object")
        .insert(field.to_owned(), value);
    serde_json::json!({"role":"user", "content":[block]})
}
