use anyhow::{Result, bail};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

use super::Session;
use super::content::{ToolResult, remember_consumed_tool_id};
use super::content_batch::is_batch_request;

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

#[path = "content_pending_batch.rs"]
mod batch;
use batch::{append_completed_batches, process_batch_result};
