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

pub(super) async fn candidate_length(
    session: &Arc<Session>,
    signature: &Arc<str>,
    messages: &[Value],
) -> Option<usize> {
    if !Arc::ptr_eq(&session.signature, signature)
        && session.signature.as_ref() != signature.as_ref()
    {
        return None;
    }
    matching_transcript_len(session, messages).await
}
