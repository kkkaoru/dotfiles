use serde_json::{Value, json};

use super::*;

fn request(content: Value) -> MessagesRequest {
    MessagesRequest {
        model: "main-model".to_owned(),
        system: Value::Null,
        messages: vec![json!({"role":"user", "content":content})],
        tools: Vec::new(),
        stream: false,
        output_config: json!({}),
        metadata: json!({}),
        working_directory: None,
        disabled_subagent_models: Default::default(),
        claudex_collaborator_model: None,
    }
}

fn launch_result(id: &str) -> Value {
    json!({
        "type":"tool_result",
        "tool_use_id":id,
        "content":[{"type":"text", "text":format!(
            "{ASYNC_LAUNCH_PREFIX}\nagentId: internal\n{BACKGROUND_MARKER}"
        )}]
    })
}

fn pure_async_launch_tool_results(message: &Value) -> Option<Vec<String>> {
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return None;
    }
    let blocks = message.get("content")?.as_array()?;
    if blocks.is_empty() {
        return None;
    }
    blocks
        .iter()
        .map(|block| {
            if block.get("type").and_then(Value::as_str) != Some("tool_result")
                || block.get("is_error").and_then(Value::as_bool) == Some(true)
            {
                return None;
            }
            let text = strict_result_text(block.get("content")?)?;
            if !text.trim_start().starts_with(ASYNC_LAUNCH_PREFIX)
                || !text.contains(BACKGROUND_MARKER)
            {
                return None;
            }
            block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

fn tool_round_ids(message: &Value) -> Option<Vec<String>> {
    let ids = message
        .get("content")?
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .map(|block| {
            block
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    (!ids.is_empty()).then_some(ids)
}

fn latest_tool_round_ids(request: &MessagesRequest) -> Option<Vec<String>> {
    request
        .messages
        .iter()
        .rev()
        .skip(1)
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .find_map(tool_round_ids)
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
        assert!(async_launch_tool_results(&message).is_none());
        assert!(pure_async_launch_tool_results(&message).is_none());
    }
    let multi_text = request(json!([{
        "type":"tool_result", "tool_use_id":"one",
        "content":[{"type":"text", "text":ASYNC_LAUNCH_PREFIX}, {"type":"text", "text":BACKGROUND_MARKER}]
    }]));
    assert_eq!(
        async_launch_tool_results(multi_text.messages.last().unwrap()),
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
        assert!(agent_tool_round_ids(&message).is_none());
    }
    let mut current = request(json!([{"type":"tool_result", "tool_use_id":"one"}]));
    current.messages.insert(
        0,
        json!({"role":"assistant", "content":[{"type":"text","text":"no tools"}]}),
    );
    assert!(latest_tool_round_ids(&current).is_none());
    assert!(latest_agent_tool_round_ids(&current).is_none());
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
        assert!(async_launch_tool_results(&message).is_none());
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
        async_launch_tool_results(&multiline),
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

#[test]
fn accepts_successful_async_launch_results_and_ignores_mixed_noise() {
    let pure = request(json!([launch_result("one"), launch_result("two")]));
    assert_eq!(
        async_launch_tool_results(pure.messages.last().unwrap()),
        Some(vec!["one".to_owned(), "two".to_owned()])
    );
    assert_eq!(
        pure_async_launch_tool_results(pure.messages.last().unwrap()),
        Some(vec!["one".to_owned(), "two".to_owned()])
    );

    let mixed_text = request(json!([launch_result("one"), {"type":"text", "text":"hi"}]));
    assert!(pure_async_launch_tool_results(mixed_text.messages.last().unwrap()).is_none());
    assert_eq!(
        async_launch_tool_results(mixed_text.messages.last().unwrap()),
        Some(vec!["one".to_owned()])
    );
    let failed = request(json!([{
        "type":"tool_result", "tool_use_id":"one", "is_error":true,
        "content":format!("{ASYNC_LAUNCH_PREFIX} {BACKGROUND_MARKER}")
    }]));
    assert!(async_launch_tool_results(failed.messages.last().unwrap()).is_none());
    let completed = request(json!([{
        "type":"tool_result", "tool_use_id":"one", "content":"finished"
    }]));
    assert!(async_launch_tool_results(completed.messages.last().unwrap()).is_none());
    let rich = request(json!([{
        "type":"tool_result", "tool_use_id":"one",
        "content":[{"type":"image"}, {"type":"text", "text":format!("{ASYNC_LAUNCH_PREFIX} {BACKGROUND_MARKER}")}]
    }]));
    assert!(async_launch_tool_results(rich.messages.last().unwrap()).is_none());
}

#[test]
fn hands_off_agent_results_even_when_the_latest_round_also_had_other_tools() {
    let mut mixed = request(json!([
        launch_result("background"),
        {
            "type":"tool_result",
            "tool_use_id":"other",
            "content":"file contents"
        }
    ]));
    mixed.messages.insert(
        0,
        json!({
            "role":"assistant",
            "content":[
                {"type":"text", "text":"Launching delegated work."},
                {"type":"tool_use", "id":"background", "name":"Agent", "input":{}},
                {"type":"tool_use", "id":"other", "name":"Read", "input":{}}
            ]
        }),
    );
    assert_eq!(
        latest_agent_tool_round_ids(&mixed),
        Some(vec!["background".to_owned()])
    );
    assert_eq!(
        exact_async_launch_acknowledgement(
            mixed.messages.last().unwrap(),
            &["background".to_owned()]
        ),
        Some(vec!["background".to_owned()])
    );
}

#[test]
fn requires_results_to_belong_to_the_latest_native_tool_round() {
    let mut correlated = request(json!([launch_result("background")]));
    correlated.messages.insert(
        0,
        json!({
            "role":"assistant",
            "content":[
                {"type":"text", "text":"Launching delegated work."},
                {"type":"tool_use", "id":"background", "name":"Agent", "input":{}},
                {"type":"tool_use", "id":"other", "name":"Read", "input":{}}
            ]
        }),
    );
    assert_eq!(
        latest_tool_round_ids(&correlated),
        Some(vec!["background".to_owned(), "other".to_owned()])
    );

    let uncorrelated = request(json!([launch_result("background")]));
    assert!(latest_tool_round_ids(&uncorrelated).is_none());
}

#[test]
fn requires_an_exact_unique_async_result_set() {
    let expected = vec!["one".to_owned(), "two".to_owned()];
    let exact = request(json!([launch_result("two"), launch_result("one")]));
    assert_eq!(
        exact_async_launch_acknowledgement(exact.messages.last().unwrap(), &expected),
        Some(vec!["two".to_owned(), "one".to_owned()])
    );

    let partial = request(json!([launch_result("one")]));
    assert!(
        exact_async_launch_acknowledgement(partial.messages.last().unwrap(), &expected).is_none()
    );
    let duplicate = request(json!([launch_result("one"), launch_result("one")]));
    assert!(
        exact_async_launch_acknowledgement(duplicate.messages.last().unwrap(), &expected).is_none()
    );
}

#[tokio::test]
async fn background_handoff_returns_visible_native_end_turn_without_lifecycle_tags() {
    let json_request = request(json!([launch_result("one")]));
    let response = internal_notification::acknowledge_with_text(
        &json_request,
        "Background agent launched; the main prompt is ready.",
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["stop_reason"], "end_turn");
    assert_eq!(
        body["content"][0]["text"],
        "Background agent launched; the main prompt is ready."
    );
    assert!(!body.to_string().contains("agent-message"));

    let mut stream_request = json_request;
    stream_request.stream = true;
    let response = internal_notification::acknowledge_with_text(
        &stream_request,
        "Background agent launched; the main prompt is ready.",
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert!(body.contains("event: message_start"));
    assert!(body.contains("content_block_delta"));
    assert!(body.contains("main prompt is ready"));
    assert!(body.contains(r#""stop_reason":"end_turn""#));
    assert!(body.contains("event: message_stop"));
}

#[test]
fn background_handoff_text_matches_the_native_launch_count() {
    assert_eq!(
        background_handoff_text(1),
        "Background agent launched; the main prompt is ready."
    );
    assert_eq!(
        background_handoff_text(3),
        "3 background agents launched; the main prompt is ready."
    );
}

#[test]
fn async_launch_results_ignore_empty_array_content() {
    assert!(
        async_launch_tool_results(&json!({
            "role":"user",
            "content":[{"type":"tool_result","tool_use_id":"x","content":[]}]
        }))
        .is_none()
    );
    assert!(strict_result_text(&json!([])).is_none());
}

#[test]
fn async_launch_results_skip_blank_tool_use_ids() {
    assert!(
        async_launch_tool_results(&json!({
            "role":"user",
            "content":[launch_result("")]
        }))
        .is_none()
    );
}
