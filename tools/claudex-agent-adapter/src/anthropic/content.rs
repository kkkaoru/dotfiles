use std::collections::HashSet;

use anyhow::Result;
use serde_json::{Value, json};

pub(super) use super::content_pending::take_pending_results;
use super::{MessagesRequest, Session};

#[path = "content_steering.rs"]
mod content_steering;
#[path = "content_tool_result.rs"]
mod content_tool_result;
pub(in crate::anthropic) use content_steering::mid_turn_user_steering;
use content_tool_result::tool_result;

// consumed IDs suppress replays of completed results.
const MAX_CONSUMED_TOOL_IDS: usize = 4_096;

pub(super) fn remember_consumed_tool_id(consumed: &mut HashSet<String>, id: String) {
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

pub(super) fn request_signature(
    request: &MessagesRequest,
    advisor_model: Option<&str>,
    collaborator_model: Option<&str>,
) -> Result<String> {
    serde_json::to_string(&json!({
        "system": canonical_value(&request.system),
        "tools": request.tools.iter().map(canonical_value).collect::<Vec<_>>(),
        "metadata": request.metadata.get("user_id"),
        "transport_identity": request.metadata.get("_claudex_transport_identity"),
        "subagent_spawn_limit_reached": request.metadata.get("_claudex_subagent_spawn_limit_reached"),
        "working_directory": request.working_directory,
        "disabled_subagent_models": request.disabled_subagent_models,
        "advisor_model": advisor_model,
        "collaborator_model": collaborator_model
    }))
    .map_err(Into::into)
}

pub(super) fn pending_request_id(pending: &Value) -> Value {
    super::agent_batch::pending_batch(pending)
        .map(|pending| pending.request_id.to_owned())
        .unwrap_or_else(|| pending.clone())
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

#[path = "content_canonical.rs"]
mod canonical;
pub(super) use canonical::{
    canonical_eq, canonical_value, image_data_url, system_text, text_block,
};
#[cfg(test)]
pub(super) use canonical::content_text;

pub(super) fn collect_tool_results(messages: &[Value]) -> Vec<ToolResult> {
    messages
        .iter()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter_map(tool_result)
        .collect()
}

/// Contiguous trailing user messages after the last non-user turn.
pub(super) fn trailing_user_messages(messages: &[Value]) -> &[Value] {
    let mut start = messages.len();
    while start > 0 && messages[start - 1].get("role").and_then(Value::as_str) == Some("user") {
        start -= 1;
    }
    &messages[start..]
}

/// Tool results across the trailing user suffix (not only `messages.last()`).
pub(super) fn collect_turn_tool_results(messages: &[Value]) -> Vec<ToolResult> {
    collect_tool_results(trailing_user_messages(messages))
}

pub(super) fn attach_mid_turn_steering(results: &mut [ToolResult], steering: &str) {
    let Some(last) = results.last_mut() else {
        return;
    };
    last.content_items.push(input_text(steering));
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

pub(super) fn input_text(text: &str) -> Value {
    json!({
        "type": "inputText",
        "text": super::team_protocol::clarify_result(text)
    })
}

#[path = "content_response.rs"]
mod content_response;
pub(in crate::anthropic) use content_response::{
    anthropic_response, estimated_block_tokens, estimated_tokens, serialized_len, sse,
};
pub use content_response::{error_response, token_count};

#[cfg(test)]
include!("content_tests.rs");
