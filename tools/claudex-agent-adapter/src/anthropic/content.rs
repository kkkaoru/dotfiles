use std::collections::HashSet;

use anyhow::{Result, bail};
use axum::{
    body::Body,
    http::{HeaderValue, Response, StatusCode, header},
};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{MessagesRequest, Segment, Session};

pub(super) struct ToolResult {
    pub(super) tool_use_id: String,
    pub(super) content_items: Vec<Value>,
    pub(super) is_error: bool,
}

pub(super) async fn take_pending_results(
    session: &Session,
    results: Vec<ToolResult>,
) -> Result<Vec<(Value, ToolResult)>> {
    let mut pending = session.pending_tools.lock().await;
    let mut consumed = session.consumed_tool_ids.lock().await;
    let unique = results
        .iter()
        .map(|result| result.tool_use_id.as_str())
        .collect::<HashSet<_>>();
    let valid = unique.len() == results.len()
        && results.iter().all(|result| {
            pending.contains_key(&result.tool_use_id)
                || consumed.contains(result.tool_use_id.as_str())
        });
    if !valid {
        bail!("Claude returned duplicate or unknown tool_use_id values");
    }
    let responses = results
        .into_iter()
        .filter_map(|result| {
            pending.remove(&result.tool_use_id).map(|id| {
                consumed.insert(result.tool_use_id.clone());
                (id, result)
            })
        })
        .collect();
    if pending.is_empty() {
        *session
            .pending_since
            .lock()
            .expect("pending tool clock poisoned") = None;
    }
    Ok(responses)
}

pub(super) fn request_signature(
    request: &MessagesRequest,
    advisor_model: Option<&str>,
    collaborator_model: Option<&str>,
) -> Result<String> {
    serde_json::to_string(&json!({
        "system": canonical_value(&request.system),
        "tools": request.tools.iter().map(canonical_value).collect::<Vec<_>>(),
        "metadata": request.metadata.get("user_id"),
        "advisor_model": advisor_model,
        "collaborator_model": collaborator_model
    }))
    .map_err(Into::into)
}

pub(super) async fn matching_transcript_len(
    session: &Session,
    messages: &[Value],
) -> Option<usize> {
    let transcript = session.transcript.lock().await;
    (transcript.len() <= messages.len()
        && transcript
            .iter()
            .zip(messages)
            .all(|(left, right)| canonical_value(left) == canonical_value(right)))
    .then_some(transcript.len())
}

pub(super) fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_value).collect()),
        Value::Object(values) => canonical_object(values),
        value => value.clone(),
    }
}

fn canonical_object(values: &Map<String, Value>) -> Value {
    Value::Object(
        values
            .iter()
            .filter(|(key, _)| key.as_str() != "cache_control")
            .map(|(key, value)| (key.clone(), canonical_value(value)))
            .collect(),
    )
}

pub(super) fn system_text(system: &Value) -> String {
    content_text(system)
}

pub(super) fn content_text(content: &Value) -> String {
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

fn text_block(block: &Value) -> Option<&str> {
    (block.get("type").and_then(Value::as_str) == Some("text"))
        .then(|| block.get("text").and_then(Value::as_str))
        .flatten()
}

pub(super) fn full_transcript_input(messages: &[Value]) -> Vec<Value> {
    if messages.len() == 1 && messages[0].get("role").and_then(Value::as_str) == Some("user") {
        return user_input_from_messages(messages);
    }
    vec![json!({
        "type": "text",
        "text": format!(
            "Continue this Claude Code conversation. The role-tagged history follows:\n{}",
            serde_json::to_string(messages).unwrap_or_default()
        )
    })]
}

pub(super) fn user_input_from_messages(messages: &[Value]) -> Vec<Value> {
    let mut input = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .flat_map(message_input)
        .collect::<Vec<_>>();
    if input.is_empty() {
        input.push(json!({ "type": "text", "text": "Continue." }));
    }
    input
}

fn message_input(message: &Value) -> Vec<Value> {
    match message.get("content") {
        Some(Value::String(text)) => vec![json!({ "type": "text", "text": text })],
        Some(Value::Array(blocks)) => blocks.iter().filter_map(input_block).collect(),
        _ => Vec::new(),
    }
}

fn input_block(block: &Value) -> Option<Value> {
    match block.get("type").and_then(Value::as_str) {
        Some("text") => Some(json!({
            "type": "text",
            "text": block.get("text").and_then(Value::as_str).unwrap_or("")
        })),
        Some("image") => image_data_url(block).map(|url| json!({ "type": "image", "url": url })),
        _ => None,
    }
}

pub(super) fn image_data_url(block: &Value) -> Option<String> {
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

pub(super) fn collect_tool_results(messages: &[Value]) -> Vec<ToolResult> {
    messages
        .iter()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter_map(tool_result)
        .collect()
}

pub(super) fn transcript_owns_tool_results(messages: &[Value], results: &[ToolResult]) -> bool {
    let tool_use_ids = messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| block.get("id").and_then(Value::as_str))
        .collect::<HashSet<_>>();
    !tool_use_ids.is_empty()
        && results
            .iter()
            .all(|result| tool_use_ids.contains(result.tool_use_id.as_str()))
}

fn tool_result(block: &Value) -> Option<ToolResult> {
    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
        return None;
    }
    let tool_use_id = block.get("tool_use_id")?.as_str()?.to_owned();
    let mut content_items = tool_result_content(block.get("content"));
    if content_items.is_empty() {
        content_items.push(json!({ "type": "inputText", "text": "" }));
    }
    Some(ToolResult {
        tool_use_id,
        content_items,
        is_error: block
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn tool_result_content(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::String(text)) => vec![input_text(text)],
        Some(Value::Array(items)) => items.iter().filter_map(tool_result_item).collect(),
        _ => Vec::new(),
    }
}

fn tool_result_item(item: &Value) -> Option<Value> {
    match item.get("type").and_then(Value::as_str) {
        Some("text") => Some(input_text(
            item.get("text").and_then(Value::as_str).unwrap_or(""),
        )),
        Some("image") => image_data_url(item)
            .map(|image_url| json!({ "type": "inputImage", "imageUrl": image_url })),
        _ => None,
    }
}

fn input_text(text: &str) -> Value {
    json!({
        "type": "inputText",
        "text": super::team_protocol::clarify_result(text)
    })
}

pub(super) fn anthropic_response(segment: Segment, model: &str) -> Response<Body> {
    json_response(json!({
        "id": format!("msg_{}", Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": segment.blocks,
        "stop_reason": segment.stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": segment.usage.input_tokens,
            "output_tokens": segment.usage.output_tokens
        }
    }))
}

pub(super) fn estimated_tokens(text: &str) -> u64 {
    u64::try_from(text.len().div_ceil(4)).unwrap_or(u64::MAX)
}

pub(super) fn estimated_block_tokens(block: &Value) -> u64 {
    block
        .get("text")
        .and_then(Value::as_str)
        .map_or(0, estimated_tokens)
}

pub(super) fn sse(event: &str, value: Value) -> String {
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
    let system = serde_json::to_string(&request.system)
        .unwrap_or_default()
        .len();
    let messages = serde_json::to_string(&request.messages)
        .unwrap_or_default()
        .len();
    let tools = serde_json::to_string(&request.tools)
        .unwrap_or_default()
        .len();
    // Codex app-server remains authoritative for the real context window and compaction.
    (system + messages + tools).div_ceil(4)
}

pub fn error_response(status: StatusCode, error: anyhow::Error) -> Response<Body> {
    tracing::error!(%error, "Anthropic compatibility request failed");
    let body = json!({
        "type":"error",
        "error":{"type":"api_error","message":error.to_string()}
    });
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("valid error response")
}

#[cfg(test)]
include!("content_tests.rs");
