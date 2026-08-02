use axum::{
    body::Body,
    http::{Response, StatusCode, header},
};
use serde_json::{Value, json};

use super::super::{
    MessagesRequest, Segment, Usage, WebEvidenceSummary,
    content::{anthropic_response, estimated_tokens, sse},
    stream::message_start,
};

const DEFAULT_NOTIFICATION_TEXT: &str = "Background task update received.";

/// Return a visible end-turn response without opening a provider turn.
///
/// Claude Code treats an empty assistant response as an incomplete turn and
/// injects a synthetic "previous response had no visible output" user message.
/// That re-enters the provider, queues the next user input, and can make
/// independent task notifications appear batched. Emit only the meaningful
/// lifecycle payload with its XML wrapper removed instead.
pub(crate) fn acknowledge(request: &MessagesRequest) -> Response<Body> {
    let text = notification_ack_text(request);
    acknowledge_with_text(request, &text)
}

pub(crate) fn acknowledge_with_text(request: &MessagesRequest, text: &str) -> Response<Body> {
    let text = if text.trim().is_empty() {
        DEFAULT_NOTIFICATION_TEXT
    } else {
        text.trim()
    };
    let input_tokens = u64::try_from(super::super::token_count(request)).unwrap_or(u64::MAX);
    let output_tokens = estimated_tokens(text);
    if !request.stream {
        return anthropic_response(
            Segment {
                blocks: vec![json!({"type":"text", "text":text})],
                stop_reason: "end_turn",
                usage: Usage {
                    input_tokens,
                    output_tokens,
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
            "content_block_start",
            json!({
                "type":"content_block_start",
                "index":0,
                "content_block":{"type":"text","text":""}
            }),
        ),
        sse(
            "content_block_delta",
            json!({
                "type":"content_block_delta",
                "index":0,
                "delta":{"type":"text_delta","text":text}
            }),
        ),
        sse(
            "content_block_stop",
            json!({"type":"content_block_stop","index":0}),
        ),
        sse(
            "message_delta",
            json!({
                "type":"message_delta",
                "delta":{"stop_reason":"end_turn","stop_sequence":null},
                "usage":{"output_tokens":output_tokens}
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

fn notification_ack_text(request: &MessagesRequest) -> String {
    request
        .messages
        .iter()
        .rev()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content"))
        .filter_map(notification_text_from_content)
        .next()
        .unwrap_or_else(|| DEFAULT_NOTIFICATION_TEXT.to_owned())
}

fn notification_text_from_content(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .filter(|text| !super::is_monitor_hint_text(text))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    if let Some(body) = lifecycle_body(&text, "agent-message") {
        return Some(sanitize_visible_text(body));
    }
    let body = lifecycle_body(&text, "task-notification")?;
    let mut parts = Vec::new();
    // Claude Code renders status in its task panel. Only the user-facing
    // summary and result belong in the assistant turn; exposing `completed`
    // or `stopped` as prose leaks transport metadata into the main session.
    for field in ["summary", "result"] {
        if let Some(value) = xml_field(body, field)
            && !value.trim().is_empty()
            && !parts.iter().any(|part: &String| part == value.trim())
        {
            parts.push(sanitize_visible_text(value));
        }
    }
    if parts.is_empty() {
        Some(sanitize_visible_text(body))
    } else {
        Some(parts.join("\n\n"))
    }
}

fn lifecycle_body<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let opening = format!("<{tag}");
    let start = text.find(&opening)?;
    let opening_end = text[start..].find('>')? + start + 1;
    let closing = format!("</{tag}>");
    let end = text[opening_end..].find(&closing)? + opening_end;
    Some(&text[opening_end..end])
}

fn xml_field<'a>(text: &'a str, field: &str) -> Option<&'a str> {
    lifecycle_body(text, field)
}

fn sanitize_visible_text(text: &str) -> String {
    text.replace("<agent-message", "agent message")
        .replace("</agent-message>", "")
        .replace("<task-notification", "task notification")
        .replace("</task-notification>", "")
        .trim()
        .to_owned()
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

    #[tokio::test]
    async fn acknowledges_internal_notification_without_echoing_it() {
        let request =
            request("<agent-message from=\"general-purpose\">worker output</agent-message>".into());
        let response = acknowledge(&request);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(!body.contains("agent-message"));
        assert!(body.contains("worker output"));
        assert!(body.contains("\"stop_reason\":\"end_turn\""));

        let mut streaming = request;
        streaming.stream = true;
        let response = acknowledge(&streaming);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(!body.contains("agent-message"));
        assert!(body.contains("worker output"));
        assert!(body.contains("content_block_delta"));
        assert!(body.contains("message_delta"));
        assert!(body.contains("message_stop"));
    }

    #[tokio::test]
    async fn acknowledges_task_notification_with_summary_and_result_without_xml() {
        let request = request(
            "<task-notification><status>completed</status><summary>Agent \"worker\" finished</summary><result>worker result</result><note>internal</note></task-notification>".into(),
        );
        let response = acknowledge(&request);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        let response: Value = serde_json::from_str(&body).unwrap();
        let text = response["content"][0]["text"].as_str().unwrap();
        assert!(!text.contains("task-notification"));
        assert!(text.contains("Agent \"worker\" finished"));
        assert!(text.contains("worker result"));
        assert!(!text.contains("completed"));
        assert!(!text.contains("internal"));
    }
}
