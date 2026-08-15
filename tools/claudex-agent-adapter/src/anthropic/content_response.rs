use std::io::Write;

use axum::{
    body::Body,
    http::{HeaderValue, Response, StatusCode, header},
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::super::{MessagesRequest, Segment};

pub(in crate::anthropic) fn anthropic_response(segment: Segment, model: &str) -> Response<Body> {
    let mut usage = json!({
        "input_tokens": segment.usage.input_tokens,
        "output_tokens": segment.usage.output_tokens,
        "server_tool_use": {
            "web_search_requests": segment.usage.web_search_requests
        }
    });
    segment.usage.apply_anthropic_details(&mut usage);
    let mut response = json!({
        "id": format!("msg_{}", Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": segment.blocks,
        "stop_reason": segment.stop_reason,
        "stop_sequence": null,
        "usage": usage
    });
    if let Some(metadata) = segment.web_evidence.metadata() {
        response["metadata"] = metadata;
    }
    json_response(response)
}

pub(in crate::anthropic) fn estimated_tokens(text: &str) -> u64 {
    u64::try_from(text.len().div_ceil(4)).unwrap_or(u64::MAX)
}

pub(in crate::anthropic) fn estimated_block_tokens(block: &Value) -> u64 {
    block
        .get("text")
        .and_then(Value::as_str)
        .map_or(0, estimated_tokens)
}

pub(in crate::anthropic) fn sse(event: &str, value: Value) -> String {
    format!("event: {event}\ndata: {value}\n\n")
}

fn json_response(value: Value) -> Response<Body> {
    let mut response = Response::new(Body::from(value.to_string()));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

pub fn token_count(request: &MessagesRequest) -> usize {
    let system = serialized_len(&request.system);
    let messages = serialized_len(&request.messages);
    let tools = serialized_len(&request.tools);
    // Codex app-server remains authoritative for the real context window and compaction.
    (system + messages + tools).div_ceil(4)
}

#[derive(Default)]
struct ByteCounter(usize);

impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(in crate::anthropic) fn serialized_len(value: &impl serde::Serialize) -> usize {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value).map_or(0, |()| counter.0)
}

pub fn error_response(status: StatusCode, error: anyhow::Error) -> Response<Body> {
    tracing::error!(%error, "Anthropic compatibility request failed");
    let status = super::super::error::http_status(status, &error);
    let error_type = super::super::error::error_type(&error);
    let body = json!({
        "type":"error",
        "error":{"type":error_type,"message":error.to_string()}
    });
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("valid error response")
}
