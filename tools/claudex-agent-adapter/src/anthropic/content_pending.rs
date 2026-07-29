use anyhow::{Result, bail};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use super::Session;
use super::content::{ToolResult, input_text, remember_consumed_tool_id};
use super::content_batch::{
    batch_progress, is_batch_request, store_batch_result, take_batch_result,
};

type AgentBatch = (Value, usize, Vec<(usize, ToolResult)>, Vec<String>);
#[derive(Clone)]
struct BatchCompletion {
    request_id: Value,
    content_items: Vec<Value>,
    is_error: bool,
    tool_use_ids: Vec<String>,
}

struct BatchCompletionContext<'a> {
    pending: &'a mut HashMap<String, Value>,
    consumed: &'a mut HashSet<String>,
    responses: &'a mut Vec<(Value, ToolResult)>,
    completed_tool_use_ids: &'a mut Vec<String>,
}

fn validate_results(
    results: &[ToolResult],
    pending: &HashMap<String, Value>,
    consumed: &HashSet<String>,
) -> Result<()> {
    let mut seen = HashSet::new();
    for result in results {
        let is_batch = pending
            .get(&result.tool_use_id)
            .is_some_and(is_batch_request);
        if !seen.insert(result.tool_use_id.as_str()) && !is_batch {
            bail!("Claude returned duplicate or unknown tool_use_id values");
        }
        if !pending.contains_key(&result.tool_use_id)
            && !consumed.contains(result.tool_use_id.as_str())
        {
            bail!("Claude returned duplicate or unknown tool_use_id values");
        }
    }
    Ok(())
}

pub(super) async fn take_pending_results(
    session: &Session,
    results: Vec<ToolResult>,
) -> Result<(Vec<(Value, ToolResult)>, Vec<String>)> {
    let mut pending = session.pending_tools.lock().await;
    let mut consumed = session.consumed_tool_ids.lock().await;
    validate_results(&results, &pending, &consumed)?;
    let mut responses = Vec::new();
    let mut completed_tool_use_ids = Vec::new();
    process_results(
        results,
        &mut pending,
        &mut consumed,
        &mut responses,
        &mut completed_tool_use_ids,
    );
    append_completed_batches(
        &mut pending,
        &mut consumed,
        &mut responses,
        &mut completed_tool_use_ids,
    );
    if pending.is_empty() {
        *session
            .pending_since
            .lock()
            .expect("pending tool clock poisoned") = None;
    }
    Ok((responses, completed_tool_use_ids))
}

fn process_results(
    results: Vec<ToolResult>,
    pending: &mut HashMap<String, Value>,
    consumed: &mut HashSet<String>,
    responses: &mut Vec<(Value, ToolResult)>,
    completed_tool_use_ids: &mut Vec<String>,
) {
    for result in results {
        if consumed.contains(result.tool_use_id.as_str()) {
            continue;
        }
        let is_batch = pending
            .get(&result.tool_use_id)
            .is_some_and(is_batch_request);
        if is_batch {
            process_batch_result(result, pending);
            continue;
        }
        // `validate_results` confirmed this ID before the mutable pass.
        let id = pending
            .remove(&result.tool_use_id)
            .expect("validated tool result must remain pending");
        remember_consumed_tool_id(consumed, result.tool_use_id.clone());
        completed_tool_use_ids.push(result.tool_use_id.clone());
        responses.push((id, result));
    }
}

fn process_batch_result(result: ToolResult, pending: &mut HashMap<String, Value>) {
    let request_id = super::agent_batch::pending_batch(
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

fn append_completed_batches(
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

fn collect_batch_results(
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

fn merge_batch_results(results: Vec<(usize, ToolResult)>) -> (Vec<Value>, bool) {
    let mut content_items = Vec::new();
    let mut is_error = false;
    for (index, result) in results {
        content_items.push(input_text(&format!("SubAgent {} result:", index + 1)));
        content_items.extend(result.content_items);
        is_error = is_error || result.is_error;
    }
    (content_items, is_error)
}

fn finish_batch(completion: BatchCompletion, context: BatchCompletionContext<'_>) {
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
