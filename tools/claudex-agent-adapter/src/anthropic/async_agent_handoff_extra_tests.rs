use serde_json::{Value, json};

use super::*;

fn request(content: Value) -> MessagesRequest {
    serde_json::from_value(json!({
        "model":"main-model",
        "messages":[{"role":"user", "content":content}]
    }))
    .expect("request")
}

#[test]
fn rejects_malformed_async_acknowledgement_shapes() {
    for message in [
        json!({"role":"assistant", "content":[]}),
        json!({"role":"user"}),
        json!({"role":"user", "content":[]} ),
        json!({"role":"user", "content":"not a block list"}),
        json!({"role":"user", "content":[{"type":"text","text":"not a result"}]}),
        json!({"role":"user", "content":[{"type":"tool_result","content":"missing id"}]}),
        json!({"role":"user", "content":[{"type":"tool_result","tool_use_id":"","content":"missing id"}]}),
    ] {
        assert!(pure_async_launch_tool_results(&message).is_none());
    }
    let multi_text = request(json!([{
        "type":"tool_result", "tool_use_id":"one",
        "content":[{"type":"text", "text":ASYNC_LAUNCH_PREFIX}, {"type":"text", "text":BACKGROUND_MARKER}]
    }]));
    assert_eq!(
        pure_async_launch_tool_results(multi_text.messages.last().unwrap()),
        Some(vec!["one".to_owned()])
    );
    let message = multi_text.messages.last().unwrap();
    assert!(exact_async_launch_acknowledgement(message, &[]).is_none());
    assert!(exact_async_launch_acknowledgement(message, &["different".to_owned()]).is_none());
    let expected = vec!["one".to_owned()];
    assert_eq!(
        exact_async_launch_acknowledgement(message, &expected),
        Some(expected)
    );
}

#[test]
fn validates_tool_round_ids_and_latest_assistant_round() {
    for message in [
        json!({"content":[]}),
        json!({"content":"not an array"}),
        json!({"content":[{"type":"tool_use", "id":""}]}),
        json!({"content":[{"type":"tool_use"}]}),
        json!({"content":[{"type":"text", "text":"no tools"}]}),
    ] {
        assert!(tool_round_ids(&message).is_none());
    }
    let mut current = request(json!([{"type":"tool_result", "tool_use_id":"one"}]));
    current.messages.insert(
        0,
        json!({"role":"assistant", "content":[{"type":"text","text":"no tools"}]}),
    );
    assert!(latest_tool_round_ids(&current).is_none());
}

#[test]
fn exhausts_async_acknowledgement_and_text_shape_boundaries() {
    for message in [
        json!({"role":"user", "content":[{"type":"tool_result", "tool_use_id":"x", "content":42}]}),
        json!({"role":"user", "content":[{"type":"tool_result", "tool_use_id":"x", "content":"launch"}]}),
        json!({"role":"user", "content":[{"type":"tool_result", "tool_use_id":"x", "content":ASYNC_LAUNCH_PREFIX}]}),
        json!({"role":"user", "content":[{"type":"tool_result", "tool_use_id":"x", "content":"Async agent launched successfully.\nnot background"}]}),
        json!({"role":"user", "content":[{"type":"tool_result", "tool_use_id":"x", "is_error":true, "content":format!("{ASYNC_LAUNCH_PREFIX}\n{BACKGROUND_MARKER}")}]}),
    ] {
        assert!(pure_async_launch_tool_results(&message).is_none());
    }
    let multiline = json!({
        "role":"user",
        "content":[{"type":"tool_result", "tool_use_id":"x", "content":[
            {"type":"text", "text":ASYNC_LAUNCH_PREFIX},
            {"type":"text", "text":BACKGROUND_MARKER}
        ]}]
    });
    assert_eq!(
        pure_async_launch_tool_results(&multiline),
        Some(vec!["x".to_owned()])
    );
    assert!(
        exact_async_launch_acknowledgement(&multiline, &["x".to_owned(), "x".to_owned()]).is_none()
    );
    assert!(exact_async_launch_acknowledgement(&multiline, &["different".to_owned()]).is_none());
    let duplicate_expected = json!({
        "role":"user",
        "content":[
            {"type":"tool_result", "tool_use_id":"x", "content":[
                {"type":"text", "text":ASYNC_LAUNCH_PREFIX},
                {"type":"text", "text":BACKGROUND_MARKER}
            ]},
            {"type":"tool_result", "tool_use_id":"y", "content":[
                {"type":"text", "text":ASYNC_LAUNCH_PREFIX},
                {"type":"text", "text":BACKGROUND_MARKER}
            ]}
        ]
    });
    assert!(
        exact_async_launch_acknowledgement(&duplicate_expected, &["x".to_owned(), "x".to_owned()])
            .is_none()
    );
    assert_eq!(
        exact_async_launch_acknowledgement(&multiline, &["x".to_owned()]),
        Some(vec!["x".to_owned()])
    );
}

#[test]
fn invalid_text_items_are_rejected_without_status_generation() {
    assert!(append_strict_result_text(&mut String::new(), &json!({"type":"image"})).is_none());
}
