use std::collections::HashMap;

use serde_json::{Value, json};

const BATCH_RESULT_KEY: &str = "__claudex_agent_batch_result";

pub(super) struct PendingBatchResult {
    pub(super) request_id: Value,
    pub(super) index: usize,
    pub(super) total: usize,
    pub(super) tool_use_id: String,
    pub(super) content_items: Vec<Value>,
    pub(super) is_error: bool,
}

pub(super) fn is_batch_request(pending: &Value) -> bool {
    super::agent_batch::pending_batch(pending).is_some()
}

/// Return the number of results already received for one outer batch call.
///
/// The adapter deliberately keeps partial batches pending: Codex app-server
/// accepts only one JSON-RPC response for the outer tool-call id. This helper
/// provides an observable progress point without sending an invalid second
/// response while a slower worker is still running.
pub(super) fn batch_progress(
    pending: &HashMap<String, Value>,
    request_id: &Value,
) -> Option<(usize, usize)> {
    let mut total = None;
    let mut completed = 0;
    for value in pending.values() {
        let Some(marker) = super::agent_batch::pending_batch(value) else {
            continue;
        };
        if marker.request_id != request_id {
            continue;
        }
        total = Some(marker.total);
        completed += usize::from(value.get(BATCH_RESULT_KEY).is_some());
    }
    total.map(|total| (completed, total))
}

pub(super) fn store_batch_result(
    pending: &mut Value,
    tool_use_id: String,
    content_items: Vec<Value>,
    is_error: bool,
) {
    if let Some(map) = pending.as_object_mut() {
        map.insert(
            BATCH_RESULT_KEY.to_owned(),
            json!({
                "tool_use_id": tool_use_id,
                "content_items": content_items,
                "is_error": is_error,
            }),
        );
    }
}

pub(super) fn take_batch_result(pending: &Value) -> Option<PendingBatchResult> {
    let marker = super::agent_batch::pending_batch(pending)?;
    let value = pending.get(BATCH_RESULT_KEY)?;
    let tool_use_id = value.get("tool_use_id")?.as_str()?;
    let content_items = value.get("content_items")?.as_array()?;
    let is_error = value.get("is_error")?.as_bool()?;
    Some(PendingBatchResult {
        request_id: marker.request_id.to_owned(),
        index: marker.index,
        total: marker.total,
        tool_use_id: tool_use_id.to_owned(),
        content_items: content_items.to_vec(),
        is_error,
    })
}
