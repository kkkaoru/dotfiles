use serde_json::Value;
use std::collections::{HashMap, HashSet};

use crate::anthropic::content::{ToolResult, input_text, remember_consumed_tool_id};
use crate::anthropic::content_batch::{batch_progress, store_batch_result, take_batch_result};

pub(super) type AgentBatch = (Value, usize, Vec<(usize, ToolResult)>, Vec<String>);
#[derive(Clone)]
pub(super) struct BatchCompletion {
    request_id: Value,
    content_items: Vec<Value>,
    is_error: bool,
    tool_use_ids: Vec<String>,
}

pub(super) struct BatchCompletionContext<'a> {
    pending: &'a mut HashMap<String, Value>,
    consumed: &'a mut HashSet<String>,
    responses: &'a mut Vec<(Value, ToolResult)>,
    completed_tool_use_ids: &'a mut Vec<String>,
}

pub(super) fn process_batch_result(result: ToolResult, pending: &mut HashMap<String, Value>) {
    let request_id = crate::anthropic::agent_batch::pending_batch(
        pending
            .get(&result.tool_use_id)
            .expect("validated batch result must remain pending"),
    )
    .expect("batch marker must remain valid")
    .request_id
    .to_owned();
    let pending_request = pending
        .get_mut(&result.tool_use_id)
        .expect("validated batch result must remain pending");
    // Keep partial members pending: app-server accepts one response for the
    // outer JSON-RPC request, so fast members cannot emit a second response.
    store_batch_result(
        pending_request,
        result.tool_use_id,
        result.content_items,
        result.is_error,
    );
    if let Some((completed, total)) = batch_progress(pending, &request_id) {
        tracing::debug!(
            completed,
            total,
            request_id = %request_id,
            "claudex batch worker result received; retaining partial batch pending"
        );
    }
}

pub(super) fn append_completed_batches(
    pending: &mut HashMap<String, Value>,
    consumed: &mut HashSet<String>,
    responses: &mut Vec<(Value, ToolResult)>,
    completed_tool_use_ids: &mut Vec<String>,
) {
    let mut batches: HashMap<String, AgentBatch> = HashMap::new();
    collect_batch_results(pending, &mut batches);
    for (_, (request_id, total, mut results, tool_use_ids)) in batches {
        if results.len() != total {
            continue;
        }
        results.sort_by_key(|(index, _)| *index);
        let (content_items, is_error) = merge_batch_results(results);
        let completion = BatchCompletion {
            request_id,
            content_items,
            is_error,
            tool_use_ids,
        };
        finish_batch(
            completion,
            BatchCompletionContext {
                pending,
                consumed,
                responses,
                completed_tool_use_ids,
            },
        );
    }
}

pub(super) fn collect_batch_results(
    pending: &HashMap<String, Value>,
    batches: &mut HashMap<String, AgentBatch>,
) {
    for (tool_use_id, pending) in pending {
        let Some(pending_result) = take_batch_result(pending) else {
            continue;
        };
        let request_id =
            serde_json::to_string(&pending_result.request_id).expect("request id is serializable");
        let batch = batches.entry(request_id).or_insert_with(|| {
            (
                pending_result.request_id.clone(),
                pending_result.total,
                Vec::new(),
                Vec::new(),
            )
        });
        batch.2.push((
            pending_result.index,
            ToolResult {
                tool_use_id: pending_result.tool_use_id,
                content_items: pending_result.content_items,
                is_error: pending_result.is_error,
            },
        ));
        batch.3.push(tool_use_id.clone());
    }
}

pub(super) fn merge_batch_results(results: Vec<(usize, ToolResult)>) -> (Vec<Value>, bool) {
    let mut content_items = Vec::new();
    let mut is_error = false;
    for (index, result) in results {
        content_items.push(input_text(&format!("SubAgent {} result:", index + 1)));
        content_items.extend(result.content_items);
        is_error = is_error || result.is_error;
    }
    (content_items, is_error)
}

pub(super) fn finish_batch(completion: BatchCompletion, context: BatchCompletionContext<'_>) {
    let BatchCompletionContext {
        pending,
        consumed,
        responses,
        completed_tool_use_ids,
    } = context;
    for tool_use_id in &completion.tool_use_ids {
        remember_consumed_tool_id(consumed, tool_use_id.clone());
        completed_tool_use_ids.push(tool_use_id.clone());
        pending.remove(tool_use_id);
    }
    let tool_use_id = completion
        .tool_use_ids
        .first()
        .expect("completed batch must contain a tool result");
    responses.push((
        completion.request_id,
        ToolResult {
            tool_use_id: tool_use_id.to_owned(),
            content_items: completion.content_items,
            is_error: completion.is_error,
        },
    ));
}
