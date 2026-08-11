use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use anyhow::{Result, bail};
use serde_json::Value;

use super::super::{
    Session,
    content::{ToolResult, matching_transcript_len},
};

mod signature;
use signature::stable_signature_identity;
#[cfg(test)]
use signature::usable_transport_identity;

pub(super) fn touch_session(session: &Session) {
    *session
        .last_activity
        .lock()
        .expect("session clock poisoned") = std::time::Instant::now();
}

pub(super) fn owns_tool_result(
    pending: &HashMap<String, Value>,
    consumed: &HashSet<String>,
    tool_use_id: &str,
) -> bool {
    pending.contains_key(tool_use_id) || consumed.contains(tool_use_id)
}

pub(super) async fn first_session_owning_results(
    sessions: Vec<Arc<Session>>,
    results: &[ToolResult],
) -> Option<Arc<Session>> {
    for session in sessions {
        if session_owns_results(&session, results).await {
            return Some(session);
        }
    }
    None
}

async fn session_owns_results(session: &Session, results: &[ToolResult]) -> bool {
    let pending = session.pending_tools.lock().await;
    let consumed = session.consumed_tool_ids.lock().await;
    results
        .iter()
        .all(|result| owns_tool_result(&pending, &consumed, &result.tool_use_id))
}

pub(super) fn is_better_length(best: Option<usize>, candidate: usize) -> bool {
    match best {
        Some(best) => candidate > best,
        None => true,
    }
}

pub(super) fn validate_tool_result_ownership(
    pending: &HashMap<String, Value>,
    consumed: &HashSet<String>,
    tool_results: &[ToolResult],
) -> Result<()> {
    if tool_results
        .iter()
        .all(|result| owns_tool_result(pending, consumed, &result.tool_use_id))
    {
        return Ok(());
    }
    bail!("Claude tool results were already consumed by another request");
}

pub(super) fn should_preempt_for_context_limit(
    input_tokens: u64,
    limit: Option<u64>,
    has_tool_results: bool,
) -> bool {
    limit.is_some_and(|limit| !has_tool_results && input_tokens >= limit)
}

pub(super) fn is_idempotent_task_lifecycle_error(content_items: &[Value]) -> bool {
    // Claude Code reports a consumed TaskStop target as an error even though
    // the requested stop is already satisfied. TaskOutput `No task found` that
    // still lists `Running background agents` is a miss, not completed output.
    content_items.iter().any(|item| {
        let Some(text) = item.get("text").and_then(Value::as_str) else {
            return false;
        };
        is_idempotent_task_lifecycle_text(text)
    })
}

fn is_idempotent_task_lifecycle_text(text: &str) -> bool {
    let normalized = text
        .replace("<tool_use_error>", " ")
        .replace("</tool_use_error>", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    // Reject shell transcripts that merely quote the phrase.
    if normalized.starts_with("shell output:") {
        return false;
    }
    if normalized.contains("running background agents:") {
        return false;
    }
    normalized.starts_with("error: no task found with id:")
        || normalized.starts_with("no task found with id:")
        || (normalized.starts_with("error: task ")
            && normalized.contains(" is not running (status: completed)"))
        || (normalized.starts_with("task ")
            && normalized.contains(" is not running (status: completed)"))
}

pub(super) async fn candidate_length(
    session: &Arc<Session>,
    signature: &Arc<str>,
    messages: &[Value],
) -> Option<usize> {
    if !signatures_compatible(session.signature.as_ref(), signature.as_ref()) {
        return None;
    }
    matching_transcript_len(session, messages).await
}

/// Exact signature first. Claude Code often rewrites `system` or tool schema
/// bodies between HTTP turns; those must not cold-start a new ACP `session/new`.
/// Identity is transport + cwd + tool *names* + routing flags, not prompt text.
fn signatures_compatible(stored: &str, requested: &str) -> bool {
    if stored == requested {
        return true;
    }
    match (
        stable_signature_identity(stored),
        stable_signature_identity(requested),
    ) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
}


#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[path = "helpers_tests.rs"]
mod tests;
