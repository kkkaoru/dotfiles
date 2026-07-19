use std::{collections::HashSet, io::Write};

use anyhow::{Result, bail};
use axum::{
    body::Body,
    http::{HeaderValue, Response, StatusCode, header},
};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{MessagesRequest, Segment, Session};

// Consumed IDs only suppress replays of completed results. A 4,096-entry replay cache is generous
// for one session while bounding tool-heavy conversations; pending results live in a separate map.
const MAX_CONSUMED_TOOL_IDS: usize = 4_096;

fn remember_consumed_tool_id(consumed: &mut HashSet<String>, id: String) {
    if consumed.contains(&id) {
        return;
    }
    if consumed.len() == MAX_CONSUMED_TOOL_IDS {
        let evicted = consumed.iter().next().cloned().expect("full replay cache");
        consumed.remove(&evicted);
    }
    consumed.insert(id);
}

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
                remember_consumed_tool_id(&mut consumed, result.tool_use_id.clone());
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
            .all(|(left, right)| canonical_eq(left, right)))
    .then_some(transcript.len())
}

fn canonical_eq(left: &Value, right: &Value) -> bool {
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

pub(super) fn serialized_len(value: &impl serde::Serialize) -> usize {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value).map_or(0, |()| counter.0)
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
