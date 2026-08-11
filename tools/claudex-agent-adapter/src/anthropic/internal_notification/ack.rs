use axum::{
    body::Body,
    http::{Response, StatusCode, header},
};
use serde_json::json;

use super::super::{
    MessagesRequest, Segment, Usage, WebEvidenceSummary,
    content::{anthropic_response, estimated_tokens, sse},
    stream::message_start,
};

pub(super) const DEFAULT_NOTIFICATION_TEXT: &str = "Background task update received.";

mod text;
use text::notification_ack_text;

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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
