//! Fast-path handling for Claude Code's internal agent notifications.
//!
//! Native background Agent completion messages can arrive as a user-shaped
//! message. They are lifecycle signals, not new user instructions, so feeding
//! them through provider routing would make the next interactive prompt wait
//! behind an unnecessary turn.

use axum::{
    body::Body,
    http::{Response, StatusCode, header},
};
use serde_json::{Value, json};

use super::{MessagesRequest, Segment, Usage, WebEvidenceSummary, content::anthropic_response};
use super::{content::sse, stream::message_start};

pub(super) fn is_internal_notification_request(request: &MessagesRequest) -> bool {
    request
        .messages
        .last()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .and_then(|message| message.get("content"))
        .is_some_and(is_internal_notification_content)
}

fn is_internal_notification_content(content: &Value) -> bool {
    match content {
        Value::String(text) => is_internal_notification_text(text),
        Value::Array(blocks) if !blocks.is_empty() => blocks.iter().all(|block| {
            block.get("type").and_then(Value::as_str) == Some("text")
                && block
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(is_internal_notification_text)
        }),
        _ => false,
    }
}

fn is_internal_notification_text(text: &str) -> bool {
    let text = text.trim();
    [
        ("<agent-message", "</agent-message>"),
        ("<task-notification", "</task-notification>"),
        ("<teammate-message", "</teammate-message>"),
    ]
    .into_iter()
    .any(|(opening, closing)| text.starts_with(opening) && text.contains(closing))
        || text.starts_with("Another Claude session sent a message")
}

/// Return an empty end-turn response without opening a provider turn.
pub(super) fn acknowledge(request: &MessagesRequest) -> Response<Body> {
    let input_tokens = u64::try_from(super::token_count(request)).unwrap_or(u64::MAX);
    if !request.stream {
        return anthropic_response(
            Segment {
                blocks: Vec::new(),
                stop_reason: "end_turn",
                usage: Usage {
                    input_tokens,
                    ..Usage::default()
                },
                web_evidence: WebEvidenceSummary::default(),
            },
            &request.model,
        );
    }
    let body = [
        message_start(&request.model, input_tokens),
        sse(
            "message_delta",
            json!({
                "type":"message_delta",
                "delta":{"stop_reason":"end_turn","stop_sequence":null},
                "usage":{"output_tokens":0}
            }),
        ),
        sse("message_stop", json!({"type":"message_stop"})),
    ]
    .concat();
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header("x-accel-buffering", "no")
        .body(Body::from(body))
        .expect("valid internal notification response")
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;
    use serde_json::json;

    use super::*;

    fn request(content: Value) -> MessagesRequest {
        MessagesRequest {
            model: "test-model".to_owned(),
            system: Value::Null,
            messages: vec![json!({"role":"user", "content":content})],
            tools: Vec::new(),
            stream: false,
            output_config: Value::Null,
            metadata: Value::Null,
            working_directory: None,
            disabled_subagent_models: Default::default(),
            claudex_collaborator_model: None,
        }
    }

    #[test]
    fn recognizes_only_pure_internal_agent_notifications() {
        assert!(is_internal_notification_request(&request(
            "<agent-message from=\"general-purpose\">worker output</agent-message>".into()
        )));
        assert!(is_internal_notification_request(&request(
            json!([{"type":"text","text":"<task-notification>done</task-notification>"}])
        )));
        assert!(!is_internal_notification_request(&request(
            "Please inspect the literal <agent-message> tag".into()
        )));
        assert!(!is_internal_notification_request(&request(json!([{
            "type":"tool_result",
            "tool_use_id":"toolu_internal",
            "content":"<agent-message>result</agent-message>"
        }]))));
    }

    #[tokio::test]
    async fn acknowledges_internal_notification_without_echoing_it() {
        let request =
            request("<agent-message from=\"general-purpose\">worker output</agent-message>".into());
        let response = acknowledge(&request);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(!body.contains("agent-message"));
        assert!(body.contains("\"stop_reason\":\"end_turn\""));

        let mut streaming = request;
        streaming.stream = true;
        let response = acknowledge(&streaming);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(!body.contains("agent-message"));
        assert!(body.contains("message_delta"));
        assert!(body.contains("message_stop"));
    }
}
